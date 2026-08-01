# Implementation Plan: #1179 Remove the dead `phone` feature and the unreachable `sync_workgroup_repos` command

Status: READY_FOR_IMPLEMENTATION

Certified by `architect` at Step 7 round 3, which is final. The round-1 digest `FD9E52B772DAAC360DA773614F957596F21BED0C44778A7B1E18422D12577AD4` and the round-2 digest `C76B945165A30A32315177C7E4D55D7C4E063FC19ED6449D4BBCE6445A5DCA18` are **superseded**. The code is done and correct; what changed across the three rounds is documentation wording that this plan specifies. Round 3 replaces the categorical storage claims in `docs/security.md:14` and `PRIVACY.md:29-31` with bounded, non-categorical text (§4.6, §4.2, and the standard in §4.6.1), rebuilds the gates around it (§9.3 rule 9), and ships both as batch 5 (§8.7). The full round-3 record is §13.5; rounds 1 and 2 are §13 and §13.4. The round-1 certification and its digest `FD9E52B772DAAC360DA773614F957596F21BED0C44778A7B1E18422D12577AD4` are **superseded**: `dev-rust-grinch`'s Step 9 review found a defect in this plan's own §4.6 text, not in the implementation. §4.6 is rewritten, §4.2's evidence table is corrected, the positive documentation gate is rebuilt, and the one-line fix ships as batch 5 (§8.7). The full round-2 record is in §13.4; round 1's enrichment resolution (`dev-rust` E1-E7, `dev-rust-grinch` G1-G7) is in §13.

**Baseline commit:** `d7285ceb7bda5259e370cc25433d1aa3293c8628` (`d7285ce`)
**Branch:** `chore/1179-remove-dead-phone-and-sync-repos`, branched from `main` @ `d7285ce`
**Issue:** https://github.com/mblua/AgentsCommander/issues/1179
**Owners:** batch 1 `dev-webpage-ui`, batches 2 and 3 `dev-rust`, batch 4 `technical-writer`. Strictly sequential, see §8.0.

Every line coordinate in this plan was read directly out of `d7285ce` by the author and independently re-derived by `dev-rust` at Step 5. Where this plan and the issue body disagree, **this plan wins**; the disagreements are enumerated in §12.

> **If you are implementing a batch, read §8 and §10 first.** Two rules matter more than the rest: run every verification grep against the **working tree**, never against `d7285ce` (§9.3 rule 1), and hand off only after your gate has passed in full (§8.3).

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

Baseline sizes of every file this plan touches, so each batch owner can confirm they are working against the same bytes:

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

**Round-2 note.** The rewritten §4.6 text is still one line replacing one line, so `docs/security.md` stays at **121**, the batch-5 follow-on (§8.7) moves no count, and this table is correct as written for all 13 rows.

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

Two consequences every batch owner must internalise:

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
| D4 | `docs/security.md:14` | replace the disk-contents enumeration item with the exact truthful text in §4.6 (one line for one line) |

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

Six decisions are the architect's, not an implementer's. All are closed here. Nothing in this plan is left to judgement at implementation time.

### 4.1 Decision: batching, ownership, and commit order (four batches, four commits, three owners)

**Rationale.** Past experience in this repo is that a session can be lost during a long `cargo` gate, and a commit survives that where a working tree does not. So every batch is committed, and the expensive Rust gate runs once per Rust batch rather than once for everything.

| Batch | Content | Owner | Gate cost | Commit message |
| --- | --- | --- | --- | --- |
| **1** | TypeScript: T1, T2, T3, T4 | `dev-webpage-ui` | seconds | `chore(#1179): remove the dead PhoneAPI and syncWorkgroupRepos TS surface (batch 1)` |
| **2** | Rust, phone chain: R1-R6 | `dev-rust` | full Rust gate, **cold build** | `chore(#1179): delete the dead phone command chain and its exclusive types (batch 2)` |
| **3** | Rust, sync wrapper: R7, R8, R9 | `dev-rust` | full Rust gate, incremental | `chore(#1179): remove the unreachable sync_workgroup_repos command (batch 3)` |
| **4** | Docs: D1, D2, D3, D4 | `technical-writer` | greps only | `docs(#1179): drop the removed phone surface and retarget the conversations references (batch 4)` |

**The order is mandatory, and TypeScript first is a correctness constraint, not a preference.** The two halves have no *compile-time* dependency, but the batch-2 and batch-3 **gates** are cross-tree by design: they search `src-tauri/` and `src/` together, so a correct Rust deletion still fails its own gate if batch 1 has not landed. Full reasoning, both independent causes, and the rejected alternative are in §8.0. The commit-before-gate rule and the handoff protocol are in §8.2 and §8.3.

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

**Superseded text, shipped by batch 4 and currently at `PRIVACY.md:29-31`. It is false as a universal claim; batch 5 replaces it:**

```markdown
### Inter-Agent Messaging

The internal messaging system between agents is **entirely local**. Messages are written as Markdown files to `messaging/` directories on your own disk, inside each workgroup and inside the Root Agent directory. No external network calls are made.
```

**Replacement, `PRIVACY.md:29-31`: use exactly this text, three lines, same position:**

```markdown
### Inter-Agent Messaging

The internal messaging system between agents is **entirely local**. The file-based path writes Markdown files into `messaging/` directories inside each workgroup and inside the Root Agent directory; other delivery paths keep message content in queues of their own. No external network calls are made.
```

**Why this changed at round 3.** Rounds 1 and 2 both judged the shipped sentence correct, and both were wrong for the same reason `docs/security.md` was wrong: it is **universal**. "Messages are written as Markdown files to `messaging/` directories" quantifies over every message. Validation found a live delivery path that produces no Markdown file at all: the opt-in control-plane API stores the body inline in `<config-dir>/api-message-bus.sqlite3` (`api/message_store.rs:15`, `:722-724`, `:785-792`) and dispatches it straight from that queue (`api/dispatcher.rs:180-190` via `WakeDeliveryOrigin::DbQueue`, `:331-337` building the `OutboxMessage` from `row.body`). One counterexample is enough to make a universal false, so the shipped line is false today. **The fix is not to enumerate the other paths; it is to stop quantifying over all messages.**

**Why this wording, clause by clause:**

| Clause | Backing fact | What would falsify it |
| --- | --- | --- |
| Heading drops "(Phone)" | Required by the issue; the feature the qualifier named is gone. | n/a |
| "**entirely local**" and "No external network calls are made" | **Preserved verbatim**, per the coordinator's round-3 instruction. This is the privacy guarantee and it remains true: the messaging system contacts no external service. The API server is inbound, opt-in, off by default, and loopback-bound by default (`api/README.md:11-16`); `PRIVACY.md`'s own "Network Features" section already scopes what leaves the machine. | AC initiating a call to an external service as part of messaging. Nothing does. |
| "The file-based path writes Markdown files into `messaging/` directories" | `phone/messaging.rs:11` `MESSAGING_DIR_NAME`, `:199-207` `build_filename` producing `"{}-{}-to-{}-{}.md"`, `:214-217` `validate_filename_shape` rejecting anything without a `.md` suffix. **Scoped to one path**, not to all messages. | A non-`.md` file written into `messaging/` by that path. A different path producing no Markdown does **not** falsify it, which is exactly the round-2 failure this rewrite removes. |
| "inside each workgroup" | `phone/messaging.rs:160-164` `messaging_dir(wg_root)` -> `wg_root.join(MESSAGING_DIR_NAME)`; the workgroup root is the `wg-<N>-*` ancestor found at `:146-157`, in the project workspace. | That directory moving. |
| "and inside the Root Agent directory" | `phone/messaging.rs:166-170` `root_messaging_dir`; the directory itself is `config/root_agent.rs:626-628` `config_dir.join(ROOT_AGENT_DIR_NAME)`, its `messaging/` created at `:722`. **This root is under the config directory, not in a workspace.** | That directory moving. |
| "other delivery paths keep message content in queues of their own" | Existential and unnamed by design. True of the API DB queue (`api/README.md:6` calls it "the durable DB queue"; `message_store.rs:785-792` stores the body) and of the caller-supplied outbox (`cli/send.rs:1077-1079`, `:1093-1098`). Naming them is #1195's job; this clause only has to be true, and it stays true if their number changes. | Nothing plausible. If every other path were removed the clause would be vacuous, not false. |
| "on your own disk" dropped from the middle sentence | Round 2 carried it there. Removed because `send --outbox` (`docs/reference/cli.md:128`) writes wherever the caller points, so a mid-sentence universal about the destination is one more thing a future feature can falsify. The locality guarantee is carried by "**entirely local**" and "No external network calls are made", both preserved verbatim, which are claims about **what AC does** rather than about where every byte ends up. | n/a. This is a removal. |
| No `~/.agentscommander/` path | **Deliberate.** The two `messaging/` roots do not share a parent: a workgroup's is in the project workspace (`phone/messaging.rs:160-164`, `mailbox.rs:717-719`), the Root Agent's is under the config directory (`config/root_agent.rs:626-628`, `:722`). Naming either root here would make the sentence false for the other half; naming both is threat-model detail. This line stays shaped by **artifact**, and `docs/security.md:14` (§4.6) carries the **roots**. `PRIVACY.md:5` makes the separate, still-true claim about configuration and session data; leave that line alone. | n/a. |

The replacement is 3 lines for 3 lines, so `PRIVACY.md` stays at 54 lines.

**Recorded dissent, since round 3 is final and this is the one thing I could not fully close.** "Entirely local" and "No external network calls are made" are preserved on the coordinator's explicit instruction, and I agree they are true of AC's own behaviour. They are, however, claims a *user* can weaken without AC changing: pointing `send --outbox` at a network share puts message JSON on a remote filesystem, and widening `apiServerBind` off loopback (`api/README.md:14-16`) lets message bodies cross a network to reach AC, inbound. Neither is AC calling an external service, so neither makes the sentence false as written. I flag it because a reader could take "entirely local" as a property of the deployment rather than of the software, and that gap belongs in **#1195**'s audit rather than in this line.

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

### 4.6 Decision: the exact `docs/security.md` replacement wording

Deleting the `conversations` token alone is not enough, and shipping that would be worse than leaving the line untouched. The surviving sentence would still assert that **teams** and **messages** live under `~/.agentscommander/`, and both claims are false as stated. Correcting one falsehood while knowingly re-publishing two others, in a security document, in the same edit, is not defensible. So the whole enumeration item is rewritten to be true.

**State of the tree, read before this wording was written.** Batch 4 landed the round-1 version of this line at `8c1ea67`, so `docs/security.md:14` currently carries the **superseded** text, not the baseline text. Both are shown below. The follow-on batch replaces line `:14` **in full**, whichever of the two it finds there; every other line of the file is untouched either way, and the file stays at 121 lines in both starting states.

**Baseline text, `docs/security.md:14` at `d7285ce`:**

```markdown
4. **The disk.** Configuration, sessions, teams, conversations, and messages all live as plain files under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`).
```

**Superseded round-1 text, `docs/security.md:14` at `8c1ea67`. Do not ship this, and do not treat its presence as done:**

```markdown
4. **The disk.** Everything is plain files, in two locations. Configuration and persisted sessions live under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`). Team configuration and inter-agent messages live in your project workspace instead: team config under `_team_<name>/`, Markdown message files under each workgroup's `messaging/` directory, and each agent replica's JSON delivery queue under its local `outbox/`.
```

**Superseded round-2 text. Never shipped, rejected at validation. Do not ship it either:**

```markdown
4. **The disk.** Everything is plain files, in two locations. Under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`): configuration, persisted sessions, the Root Agent directory `ac-root-agent/` with its own `messaging/` Markdown message files and `outbox/` delivery queue, and the running instance's JSON delivery queue under `instances/<instance-id>/outbox/`. In your project workspace: team configuration under `_team_<name>/`, each workgroup's `messaging/` Markdown message files, and each agent replica's JSON delivery queue under its own `outbox/`. **Inter-agent messages live in both locations**, so securing, backing up, or erasing them means covering both.
```

**Replacement, `docs/security.md:14`: use exactly this text, one line for one line:**

```markdown
4. **The disk.** AC keeps its state in files on this machine. Under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`) you will find configuration, persisted sessions, the Root Agent directory `ac-root-agent/` including its `messaging/` Markdown message files, and the running instance's delivery queue at `instances/<instance-id>/outbox/`. In your project workspace you will find team configuration under `_team_<name>/`, each workgroup's `messaging/` Markdown message files, and each agent replica's delivery queue at `<replica>/<instance-dir>/outbox/`, where `<instance-dir>` is that replica's dot-prefixed per-instance directory. **Message data is written under both roots**, optional features and per-call overrides can put it outside either, and neither list is exhaustive, so treat this as where to start looking rather than as an inventory of everything AC writes.
```

### 4.6.1 Why two rewrites failed, and what changed in the standard

Rounds 1 and 2 both failed the same way, and it is worth stating precisely because the round-3 text is built to be immune to it rather than merely to be more complete.

**Round 1** said messages live in the project workspace "**instead**". False: Root Agent `messaging/` and the instance app outbox are under the config directory.

**Round 2** said "**Everything** is plain files, **in two locations**" and enumerated what each holds. Validation found three things wrong with it:

1. A fifth message store nobody had audited, `<config-dir>/api-message-bus.sqlite3`, holding queued message bodies inline. Re-verified: `api/message_store.rs:15` `DB_FILENAME`, `:722-724` `config_dir().join(DB_FILENAME)`, `:785-792` `messages.body TEXT NOT NULL`; `api/dispatcher.rs:180-190` delivers via `WakeDeliveryOrigin::DbQueue` and `:331-337` builds the `OutboxMessage` straight from `row.body`, so **no Markdown file is created at all** on that path; `api/README.md:150-157` calls the database plaintext and sensitive and warns that WAL can retain historical body bytes.
2. **My "`ac-root-agent/` ... `outbox/` delivery queue" claim was wrong, and I withdraw it.** Re-verified: the direct child comes from the generic Agent Matrix layout (`entity_creation.rs:128` `AGENT_MATRIX_DIRS`, `:131-134`) and has no production reader or writer; the live queue is `<root>/<agent-local-dir>/outbox/` (`cli/send.rs:827`, `:1091`), which is exactly what `phone/mailbox.rs:2761-2769` scans, alongside the app outbox and nothing else. My round-2 evidence row asserted the phrase was "true on both readings"; it was true on neither. This is the **same failure mode as round 1**: a conclusion defended by a reason I had not actually checked.
3. `send --outbox` is a public documented option (`docs/reference/cli.md:128`) that accepts any path and writes the message JSON there (`cli/send.rs:1077-1079`, `:1093-1098`), so it can place a queue outside both roots.

**The pattern.** Each round found one more omission because the sentence was **categorical**. "Everything." "In two locations." Under a universal quantifier, any single unaudited artifact makes the whole sentence false, so the only correct categorical text is an exhaustive one, and exhaustiveness here is a full security-documentation audit rather than a one-line fix. That audit is issue **#1195**, filed with the validation evidence.

**The round-3 standard, set by the user and relayed by the coordinator: bounded and non-categorical.** Describe the default AC-managed locations without asserting they are the only ones. The acceptance test applied to every clause below is: **would this sentence still be true if a sixth store existed that nobody has audited?** If no, it was rewritten. That is why the replacement uses "you will find" rather than "everything is", states outright that neither list is exhaustive, and names overrides and optional features as a category without naming the API database or `--outbox`, which belong to #1195.

**Why this wording, clause by clause.** Every clause was re-derived from the tree for round 3, and each row states what would falsify it.

| Clause | Backing fact | What would falsify it |
| --- | --- | --- |
| "AC keeps its state in files on this machine" | Every artifact named below, plus every store found during three rounds of audit including the API message database, is a local file. `config/mod.rs:169-176` resolves the config root; nothing in the messaging or persistence path writes to a remote service. | A store AC keeps somewhere other than the local filesystem. None was found, and the product is a local desktop app (threat-model item 3 already scopes the optional network endpoints). **Note it does not say "only" files, or "plain" files**: the API database is a file too, and this clause survives it. |
| "you will find" (both lists) | Existential, not universal. Each named artifact is verified in its own row below. | Only a named artifact **not** being there. A store the lists omit does **not** falsify it. This is the single most important word choice in the line, and it is what rounds 1 and 2 got wrong. |
| "Configuration ... under `~/.agentscommander/`" | `config/settings.rs:1733` `super::config_dir().map(\|d\| d.join("settings.json"))`. | `settings.json` moving out of the config directory. |
| "persisted sessions" | `config/sessions_persistence.rs:271` `super::config_dir().map(\|d\| d.join("sessions.json"))`. | `sessions.json` moving out of the config directory. |
| portable-instance parenthetical | Preserved verbatim from the baseline line. `config/mod.rs:108-135`: the portable config dir is `<binary_parent_dir>/.<binary_file_stem>/`, with a `$HOME` fallback at `:137-142`. | The portable resolution being removed. |
| "the Root Agent directory `ac-root-agent/`" | `config/root_agent.rs:13` `pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";` and `:626-628` `display_path(&config_dir.join(ROOT_AGENT_DIR_NAME))`. A **config-directory** child, not a workspace child. | The Root Agent root moving, or being renamed. |
| "including its `messaging/` Markdown message files" | `config/root_agent.rs:722` creates `root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME)`; `phone/messaging.rs:166-170` `root_messaging_dir`; `cli/send.rs:943-946` routes a Root Agent sender's `--send` file through it; `phone/messaging.rs:199-207` `build_filename` produces `.md`; `:214` `validate_filename_shape` requires the `.md` suffix. "**including**" is deliberate: the directory holds more than this. | A non-`.md` file being written **into `messaging/`**. Messages delivered by some other path that never touch `messaging/` do not falsify it, because the clause is scoped to that directory's contents. |
| "the running instance's delivery queue at `instances/<instance-id>/outbox/`" | `lib.rs:968-969` `config_dir.join("instances")`; `:996-998` `instances_dir.join(&instance_id).join("outbox")`, created at boot; `cli/send.rs:1080-1088` selects it for root/master-token sends; `:1098` writes `<msg_id>.json`; `phone/mailbox.rs:2761-2769` scans it. **Round 2's "JSON" qualifier is dropped**: the payload shape is not what a reader securing the directory needs, and dropping it removes one more thing that a future format change would falsify. | That path no longer being created or scanned. |
| "team configuration under `_team_<name>/`" | `entity_creation.rs:846`, `:917`, `:937`, `:970`, `:3025` and `cli/workgroup.rs:180` all build `workspace_dir.join(format!("_team_{}", team_name))`; `read_team_config` takes a `workspace_dir`, not a config dir. **No `teams.json` file exists**: that name occurs exactly once in `src-tauri/src/`, at `cli/send.rs:22`, as prose inside the CLI help text, and no code constructs the path. | Team config being read from the config directory. |
| "each workgroup's `messaging/` Markdown message files" | `phone/messaging.rs:11` `MESSAGING_DIR_NAME = "messaging"`, `:146-157` `workgroup_root` walks up to the `wg-<N>-*` ancestor, `:160-164` `messaging_dir(wg_root)`, `:199-207` `build_filename`. Same scoping as the Root Agent row: a claim about that directory's contents, not about all messages. | A non-`.md` file written into a workgroup `messaging/`. |
| "each agent replica's delivery queue at `<replica>/<instance-dir>/outbox/`, where `<instance-dir>` is that replica's dot-prefixed per-instance directory" | `cli/send.rs:827` `PathBuf::from(&root).join(crate::config::agent_local_dir_name())` and `:1091` `ac_dir.join("outbox")`; `config/mod.rs:165-167` `agent_local_dir_name()` returns `format!(".{}", local_dir_stem)`, hence dot-prefixed and per-instance; `phone/mailbox.rs:2761-2769` scans exactly `<path>/<agent-local-dir-name>/outbox`; `:717-719` records the full shape. `cli/purge_wg.rs:173-174`, `cli/raise_hand.rs:55`, `cli/close_session.rs:207` build the same path. **This corrects round 2**, which wrote "under its own `outbox/`" and so named the provisioned direct child `<replica>/outbox/` (`entity_creation.rs:129` `AGENT_REPLICA_DIRS`) that no production reader or writer touches. | The poller scanning a different path, or the queue moving to the direct child. |
| "**Message data is written under both roots**" | Config root: `config/root_agent.rs:722` (Root Agent `messaging/`) and `lib.rs:996-998` (instance outbox). Workspace root: `phone/messaging.rs:160-164` (workgroup `messaging/`) and `cli/send.rs:827`/`:1091` (replica queue). Existential in both directions, so a further store under either root only adds to it. | Message data ceasing to be written under one of the two roots. **Not** falsified by a store under a third root, which the next clause covers explicitly. |
| "optional features and per-call overrides can put it outside either" | Verified concretely, though deliberately not named in the line: `send --outbox` is public (`docs/reference/cli.md:128`), accepts any path, and writes there (`cli/send.rs:1077-1079`, `:1093-1098`); and the opt-in control-plane API stores message bodies in `<config-dir>/api-message-bus.sqlite3` (`api/message_store.rs:15`, `:722-724`, `:785-792`) and dispatches from that queue without creating a `messaging/` file (`api/dispatcher.rs:180-190`, `:331-337`). The clause is a **category**, so it stays true if either mechanism changes and it is already true of any third one. | Nothing plausible. Removing every override and optional store would make it vacuous rather than false, and it would still be safe to leave in a threat model. |
| "neither list is exhaustive" | Demonstrably true today: the API message database is under the config root and is not in the config list; `--outbox` targets are in neither. It is also the clause that makes the two enumerations **existential by construction**. | Nothing. A clause that disclaims completeness cannot be falsified by finding something else. **This is the round-3 fix.** |
| "treat this as where to start looking rather than as an inventory of everything AC writes" | Sets the reader's contract explicitly, so a security-conscious reader knows to keep auditing rather than to trust the list as closed. Pairs with #1195, which is the audit itself. | Nothing. It is an instruction, not a factual claim. |
| `conversations` dropped | The store is written only by `phone/manager.rs`, which R2 deletes. | A surviving writer of `<config-dir>/conversations/`. |

**What this line deliberately does not say.** It does not say "everything", "only", "all", "in two locations", or "instead". It does not name the API message database or `--outbox`: the coordinator scoped those to **#1195**, and naming them would re-open the exhaustiveness trap this rewrite exists to escape. It does not claim every inter-agent message is a Markdown file. It does not characterize the Root Agent's own queue at all, which is the legitimate omission the coordinator offered, and it is the safer choice after getting that path wrong once.

**Scope note, flagged rather than buried.** This edit corrects two pre-existing falsehoods in the baseline line (teams and messages), where the coordinator's original Step 7 instruction named only messages. The widening is deliberate and stays inside the same single line: no adjacent line is touched, and the file stays at 121 lines. Round 3 widens nothing further; it replaces claims with weaker true ones and adds no new path.

**Consistency check this closes.** `PRIVACY.md` (§4.2, also rewritten at round 3) and this line now describe one storage model at the same level of confidence: the filesystem protocol writes Markdown into `messaging/` directories under both roots, other delivery paths exist and stay local, and neither document claims its list is complete. Neither can be falsified by the other, and neither is falsified by #1195's findings.

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

**D2. `PRIVACY.md:29-31`**: replace with the exact three lines given in Section 4.2. 54 lines, unchanged. **Round 3: use §4.2's round-3 text, not the version currently in the tree.** Batch 4 shipped the round-1 wording, which validation found false as a universal claim: it says every message is a Markdown file in `messaging/`, and the control-plane API path produces no such file. D2 is now implemented by **batch 5** (§8.7) alongside D4.

**D3. `docs/agents/inter-agent-messaging.md`: delete lines `:129-132`** (heading, blank, paragraph, blank) per Section 4.3. 218 -> 214 lines.

**D4. `docs/security.md:14`**: replace the threat-model enumeration item with the exact truthful text specified in **§4.6**. One line for one line; no adjacent line is touched; the file stays at 121 lines.

**This is a rewrite, not a token deletion.** Deleting `conversations, ` alone would leave the line still claiming that teams and messages live under `~/.agentscommander/`, and §4.6 shows both claims are false as stated. Use §4.6's replacement text verbatim; it is a decided wording with per-clause evidence, not a draft.

**Round 3: use §4.6's round-3 text, not the version currently in the tree and not round 2's.** Batch 4 landed the round-1 wording, which review found false for Root Agent messaging. Round 2's replacement was rejected at validation for being categorical, for mislabelling the Root Agent's provisioned `outbox/` as a delivery queue, and for omitting a fifth message store. D4 is now implemented by **batch 5** (§8.7) alongside D2, replacing line `:14` in full.

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
| The four command names appearing in this plan file, which is tracked and matches every removed symbol | **Pathspec is the protection, not the revision.** Every per-batch grep is scoped to `src/`, `src-tauri/`, `docs/`, `PRIVACY.md` or `docs/security.md`, none of which can reach `plans/`. The two repo-wide sweeps carry `':!plans/'`. §9.3 rule 2. |
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
| `tsc` and `npm test` **pass** but the grep in §9.5 fails | The gates cannot see orphaned TS symbols (§2.4) | This is the expected failure signature for a skipped T2 or T3. Apply the missing edit. **A green gate is not evidence here.** |
| An acceptance grep returns a `plans/` hit | A repo-wide sweep lost its `':!plans/'` exclusion. This file is tracked and matches every removed symbol | Re-run per §9.4. Note the per-batch greps are pathspec-scoped and cannot reach `plans/` at all (§9.3 rule 2) |
| **Every completeness grep reports the symbols you just deleted, and the line counts all show the pre-change number** | The grep was pinned to `d7285ce`. That reads the tree **before** your edits and can neither pass nor fail | Drop the revision. Run against the working tree. §9.3 rule 1. **This is not broken work; nothing you did is wrong.** |
| **Batch 2:** `-w PhoneMessage -- src-tauri/ src/` returns 3 hits at `ipc.ts:28`, `ipc.ts:811`, `types.ts:981`; or `-w AgentInfo` returns 7 instead of 4 | **Batch 1 has not landed.** The Rust deletion is correct; the gate is cross-tree by design | Stop. Do not edit `src/` to make your gate pass, that is batch 1's scope. Report that the entry precondition was not met. §8.0 and §8.1 |
| **Batch 3:** `-w sync_workgroup_repos -- src-tauri/ src/` returns 2 hits, the second at `ipc.ts:1074` | **Batch 1 has not landed.** Same cause and same remedy as the row above | Stop and report. Do not edit `src/` |
| `git ls-tree ... -- <path>` exits 128 with `fatal: Not a valid object name` | The revision was dropped from an `ls-tree`; it requires a tree-ish and cannot become a working-tree check | Use the `test ! -e` / `test -z "$(...)"` forms in §9.4 verbatim. §9.3 rule 5 |
| An `ls-tree` check "passes" with exit 0 but you never saw its output | `git ls-tree HEAD -- <missing-path>` exits **0** with empty output. Exit status never proves absence | Assert on output, not exit status. §9.3 rule 5 |
| Criterion 10 fails with lowercase "conversation" hits in `phone/mailbox.rs` | `-i` was added to the `Conversation` grep | Remove `-i`. The case-sensitive form is correct and deliberate. §9.3 rule 4 |
| The scope gate lists a 14th path | An unrelated file was edited, or a stray file was committed | Revert it. §8.6 rule 5. If the edit was needed, that is new information: stop and report |

---

## 7. Compatibility, performance, and security

**IPC compatibility.** The Tauri IPC surface shrinks by five commands: `phone_send_message`, `phone_get_inbox`, `phone_list_agents`, `phone_ack_messages`, `sync_workgroup_repos`. All five are unreachable today from every client plane, verified independently for each: the desktop frontend (`src/`), the web dispatcher (`web/commands.rs`, zero case-insensitive `phone` hits), the control-plane API (`api/`), and the CLI (`cli/`). **No client contract is broken because no client holds one.**

**Rust API compatibility.** `agentscommander_lib` is a library crate, so removing `pub` items is technically a breaking change for any external consumer. **There is no supported or known external consumer:** the crate is unpublished, it is consumed inside this workspace only by its own binaries and by `src-tauri/tests/`, §2.3b confirms no test reaches the removed surface, and no public client contract documents any of these items. That is the strongest claim the evidence supports. It is deliberately not phrased as proof that no private fork or unpublished dependent exists anywhere, which is unknowable from this repository.

**Persistence and data.** Nothing writes `<config-dir>/conversations/` after this change. Existing directories are left untouched (§6.2). No schema, no migration, no config-format change. **This change removes one store and touches no other.** Among the stores that stay untouched: the `messaging/` directories in both roots (`<workgroup-root>/messaging/` and `<config-dir>/ac-root-agent/messaging/`), the outbox directories (`<replica-root>/<local-dir>/outbox/` and `<config-dir>/instances/<instance-id>/outbox/`), the control-plane API message database (`<config-dir>/api-message-bus.sqlite3`), `settings.json`, `sessions.json`, and team configuration under `<workspace>/_team_<name>/`. **That list is illustrative, not an inventory**: round 3 established that a closed enumeration of AC's stores does not yet exist, which is the subject of issue #1195.

**Performance.** Neutral. Five fewer `invoke_handler` entries is not a measurable difference. Marginally smaller binary.

**Security.** Net positive, though small. Each registered Tauri command is reachable from any page loaded in the webview; removing five unreachable ones removes five entry points. Of the five, `phone_send_message` is the one that actually mattered: it wrote attacker-influenced content to `<config-dir>/conversations/*.json` with a `can_communicate` check as the only gate, and nothing in the product ever needed it. Attack surface strictly shrinks; no permission, no authentication path, and no capability grant changes.

**Security and privacy documentation.** `PRIVACY.md` retains its guarantee that inter-agent messaging is entirely local with no external network calls; that clause is preserved verbatim through every round (§4.2). What changed is everything around it. The baseline `docs/security.md:14` told a security-conscious reader that team configuration and inter-agent messages live under `~/.agentscommander/`: team configuration does not live there at all, and messages live there only in part. Round 1 over-swung and asserted the mirror image. Round 2 named both roots and was still categorical, which validation falsified with a store neither round had audited. **Round 3 stops making the claim categorical at all** (§4.6, §4.2): both documents now describe the default AC-managed locations, name the file-based path as one path rather than as all messaging, and say outright that their lists are not exhaustive. Batch 5 (§8.7) ships both.

The honest summary of what this change delivers on the documentation side is therefore narrower than round 2 claimed, and it is worth stating plainly: **after batch 5 the two documents are true, mutually consistent, and explicitly incomplete.** They point a reader at the right directories to start from and tell them the list has a boundary. They are not a complete inventory of everything AC writes; three rounds established that no such inventory exists yet, and producing one is issue **#1195**.

---

## 8. Implementation order, owners, and handoff

Phase order per the planning rules: MVP -> Full Features -> Polish -> Extras. This change is small enough that MVP and Full Features are the same four batches; Polish and Extras are empty by design.

### 8.0 Three owners, strictly sequential. This is a hard constraint.

**This plan is executed by three different agents, each starting from a purged context and reading only this file.**

| Batch | Content | Owner |
| --- | --- | --- |
| 1 | TypeScript (`src/`) | `dev-webpage-ui` |
| 2 | Rust, phone chain (`src-tauri/`) | `dev-rust` |
| 3 | Rust, `sync_workgroup_repos` (`src-tauri/`) | `dev-rust` |
| 4 | Documentation | `technical-writer` |

**The order 1 -> 2 -> 3 -> 4 is mandatory, not preferred. Exactly one owner works in the tree at a time.** Two independent reasons, either of which alone is sufficient:

1. **One clone.** The workgroup has a single working copy of the repository. Two owners editing concurrently corrupt each other's batch and make `git status` meaningless as a handoff signal.
2. **The per-batch gates are cross-tree, by design.** Batch 2's and batch 3's completeness greps deliberately search **both** `src-tauri/` and `src/`, because that is what makes criteria 9, 11 and 12 whole-repository facts rather than per-language ones. A consequence is that a **correct** Rust deletion still fails its own gate if batch 1 has not landed:
   - Run batch 2 before batch 1 and `git grep -w PhoneMessage -- src-tauri/ src/` returns the three surviving TypeScript hits (`ipc.ts:28`, `ipc.ts:811`, `types.ts:981`). `-w AgentInfo` returns 7 rather than the required 4.
   - Run batch 3 before batch 1 and `git grep -w sync_workgroup_repos -- src-tauri/ src/` returns 2 hits rather than 1, the second being `ipc.ts:1074`.

   §6.4 records both signatures so a mis-ordered run is diagnosed rather than mistaken for broken Rust work.

**Decision, recorded so it is not reopened: the gates stay cross-tree and the order carries the constraint.** The alternative, narrowing each batch's greps to its own language and making only the final sweep cross-tree, was considered and rejected. It would buy nothing, because reason 1 already forces strict sequencing, and it would cost the cumulative property that makes each batch's gate assert the whole-repository state reached so far.

### 8.1 The per-batch protocol every owner follows

Each batch runs the same five steps in this order.

1. **Entry precondition.** Verify the previous batch actually landed *and* passed its gate. This is a hard gate, not a courtesy: run the previous batch's completeness greps from §9.4 yourself, plus `git status --porcelain` (must be empty) and `git log --oneline -1` (must show the previous batch's commit message). Batch 1's entry precondition is `HEAD` at the plan commit with a clean tree. **If the entry precondition fails, stop and report. Do not start your batch on top of an unverified one.**
2. **Apply the edits** from §5, in the order that batch specifies.
3. **Commit immediately, before gating.** See §8.2.
4. **Run the full gate** for that batch from §9.4.
5. **Hand off** only after the gate has passed in full. See §8.3.

### 8.2 Commit before gating, not after

**Apply the batch's edits, commit, then run the gate.** If the gate fails, fix and `git commit --amend`, or add a fixup and squash it into the batch commit **before handoff**. The result is the same four commits with the same messages and the same revert granularity.

The reason is the one §4.1 gives for committing per batch at all: a session can be lost during a long gate, and a commit survives that where a working tree does not. Gating first would leave the tree unprotected for exactly the interval the rule exists to cover. This matters most for batch 2, which is six edits deep, whose partially-applied state cannot be reconstructed by inspection, and which is immediately followed by the cold build described in §8.4.

### 8.3 A commit is not evidence its gate passed

This is the trap that commit-before-gate introduces, and it is sharper here because the batches cross agents. An owner can commit, start a long gate, and lose the session. The next owner then sees the exact expected commit and a clean tree, and can begin work on code that was never accepted.

**Three rules close it:**

1. **No handoff until the owner reports an explicit, complete gate pass.** The presence of the commit is never the signal. The report is.
2. **After a session loss following a commit, the recovering owner reruns the entire batch gate on that `HEAD`.** Do not assume it passed because the commit exists. Do not resume mid-gate.
3. **After a gate failure, the same owner fixes it, amends or squashes into the batch commit, and reruns the full gate before handing off.** Do not create a fifth implementation commit, and do not hand a failing batch forward.

A failure whose signature is not in §6.4 remains stop-and-report, per §8.6 rule 2.

### 8.4 Batch 2's gate is a cold build. Budget for it.

`src-tauri/target/` in this replica contains only `fw/` (about 2.1 GB, written by something that set a non-default target directory). There is no `src-tauri/target/debug/`, and neither `src-tauri/.cargo/config.toml` nor `CARGO_TARGET_DIR` redirects the default. So batch 2's first `cargo check --all-targets` compiles the whole dependency tree from scratch, including `rusqlite` with `features = ["bundled"]`, which is a full C compile of SQLite, plus the tauri, axum and reqwest trees. `cargo test --lib --bins --tests` then pays full codegen and links 21 binaries on Windows.

**Do not read a long first gate as a hang.**

**Batch 3's gate is incremental and cheap, conditional on the target directory surviving between the two batches.** Between batch 2 and batch 3: no `cargo clean`, no toolchain switch, and do not run the gates from a different directory or with a different `CARGO_TARGET_DIR`. If that condition breaks, §4.1's "one extra gate run" becomes a second cold build.

### 8.5 MVP / Full Features: the four batches

**Batch 1: TypeScript.** Owner `dev-webpage-ui`.
Entry precondition: clean tree at the plan commit.
Edits: T4, T1, T2 in `src/shared/ipc.ts` (in that order, bottom-up), then T3 in `src/shared/types.ts`.
Commit: `chore(#1179): remove the dead PhoneAPI and syncWorkgroupRepos TS surface (batch 1)`
Gate: `npm run typecheck`, `npm test`, plus the batch-1 block in §9.4.
Handoff: to `dev-rust` after the gate passes in full.

**Batch 2: Rust, the phone chain. Atomic.** Owner `dev-rust`.
Entry precondition: batch 1 committed and gate-passed; batch 1's greps rerun clean.
Edits: apply **all six** of R1, R2, R3, R4, R5, R6 before running anything. Intermediate states do not compile and their failures carry no information.
Commit: `chore(#1179): delete the dead phone command chain and its exclusive types (batch 2)`
Gate: from `src-tauri/`: `cargo check --all-targets`, then `cargo clippy --all-targets -- -D warnings`, then `cargo test --lib --bins --tests`. Plus the batch-2 block in §9.4.
Handoff: stays with `dev-rust` for batch 3.

**Batch 3: Rust, `sync_workgroup_repos`.** Owner `dev-rust`.
Entry precondition: batch 2 committed and gate-passed.
Edits: R7, then R8 (**locate by anchor text; the line is now `:2663`, not `:2667`**), then R9.
Commit: `chore(#1179): remove the unreachable sync_workgroup_repos command (batch 3)`
Gate: the same three `cargo` commands, plus the batch-3 block in §9.4.
Handoff: to `technical-writer` after the gate passes in full.

**Batch 4: Documentation.** Owner `technical-writer`.
Entry precondition: batch 3 committed and gate-passed.
Edits: D1 bottom-up (17 edits), then D2, D3, D4.
Commit: `docs(#1179): drop the removed phone surface and retarget the conversations references (batch 4)`
Gate: the batch-4 block in §9.4, then the **Final** block, which includes the scope gate of §9.4.
Handoff: back to the coordinator. Batch 4's owner runs the Final block because they are last; it is a whole-change gate, not a documentation gate.

### Polish

None. There is no follow-up cleanup this change defers.

### Extras

None. Everything the issue asks for lands in the four batches above.

### 8.6 Rules that hold across all batches

1. **Commit before gating, hand off only after the gate passes.** §8.2 and §8.3.
2. **Never fix a gate failure by widening the scope.** Every legitimate failure has a listed cause in §6.4. A failure that is not on that list is new information: stop and report it rather than improvising a fix.
3. **Do not reformat.** No `cargo fmt` over untouched regions, no prettier pass, no import reordering. Every edit in §5 is a deletion or a named-token replacement. The scope gate in §9.4 will catch a reformat as an out-of-whitelist change or as a line-count mismatch.
4. **Run every verification grep against the working tree**, never against `d7285ce`. §9.3 rule 1.
5. **Touch only the 13 files in the §1.3 table.** Nothing else, for any reason.

### 8.7 Batch 5: the documentation-accuracy correction. Added at round 2, widened to two files at round 3.

**Batches 1 through 4 already landed and the code is correct.** The Step 9 review confirmed every R1-R9, T1-T4 and D1-D4 cut against §5 and found no code defect; the branch is clean. What failed review is **wording this plan specified**, which batch 4 shipped faithfully. So this is not a re-run of batch 4 and not a revert: it is a small edit on top of it, in two files that batch 4 already touched.

**Batch 5: the corrected `docs/security.md:14` and `PRIVACY.md:29-31`.** Owner `technical-writer`.
Entry precondition: `bce5b31` (or later) checked out, tree clean, and this plan re-certified at Step 7 round 3.
Edits, two files, both already on the §9.4 whitelist:
1. Replace `docs/security.md:14` in full with §4.6's **round-3** replacement text. One line for one line.
2. Replace `PRIVACY.md:29-31` in full with §4.2's **round-3** replacement text. Three lines for three lines.

**Nothing else, in either file, for any reason.**
Commit: `docs(#1179): correct the disk-location and messaging accuracy claims (batch 5)`
Gate: the batch-4 block in §9.4, then the **Final** block including the scope gate. The batch-4 block is rerun whole rather than only its new greps, because D1 and D3 must still hold after these edits.
Handoff: back to the coordinator.

**Three things this batch does not change.** The final scope gate still lists exactly the same 13 paths: both files were already on the whitelist and batch 5 only edits them again. `docs/security.md` stays at 121 lines and `PRIVACY.md` at 54, so criterion 22 is unaffected. And criterion 26's four-commit assertion still holds for batches 1-4; batch 5 adds a fifth implementation commit, which is expected rather than a scope violation.

**If either target does not hold the text you expect.** For `docs/security.md:14` three superseded versions are on record in §4.6: the baseline, the round-1 text that batch 4 shipped, and the round-2 text that was never shipped. For `PRIVACY.md:29-31` two are on record in §4.2: the baseline and the shipped round-1/2 text. The replacement is the same whichever you find. **Any state not on those lists means someone edited outside this plan: stop and report it.** Do not merge, do not adapt the wording, do not treat a partial match as done.

---

## 9. Tests and objective acceptance criteria

### 9.1 No new test is written, and none is deleted

The deleted code has no test of any kind: `commands/phone.rs` and `phone/manager.rs` contain no `#[cfg(test)]` module (verified by reading both files in full), no integration test under `src-tauri/tests/` reaches them (§2.3b), and no frontend test mentions any phone symbol. **This is baseline evidence, not an acceptance check**, which is why it is the one command in §9 that is legitimately pinned to `d7285ce`: it proves there was nothing to delete before the work started.

```
$ git grep -n -i phone d7285ce -- '*.test.ts' '*.test.tsx' '*.spec.ts' '*.d.ts' '*.stories.tsx'
   -> exit 1, no hits
```

`phone/types.rs`'s test module at `:809-913` survives untouched and must keep passing; it exercises only `OutboxMessage` and `PtyInput*`. `npm run test:debt` is unaffected: no ignored or placeholder test is added or removed.

**Source-introspection tests are the one non-obvious hazard, and all are clean.** This crate contains tests that read production `.rs` and `.ts` files off disk and assert on their contents. They are invisible to symbol greps and are the likeliest source of a gate failure §6.4 would not otherwise explain. Every one was checked:

| Test | What it reads | Why this change cannot break it |
| --- | --- | --- |
| `lib.rs:3241` | `src/lib.rs`, split at `#[cfg(test)]` | Compares byte offsets of two anchors that both sit around `:1200`, far above the `invoke_handler` at `:2532`. Deleting 5 registrations does not reorder them |
| `config/local_config_io.rs:429` | walks `src/config` **and `src/commands`** | Offender-detection (`assert!(offenders.is_empty())`), not exhaustiveness, so deleting a file cannot add an offender. Its allowlist names `entity_creation.rs` only for `write_team_config`, `create_new_team_config_on_disk` and `team_dir.join("config.json")`; the wrapper R7 deletes contains none |
| `tests/pty_writer_inventory.rs:67` | every `.rs` under `src/`; permits `src/phone/mailbox.rs` | Neither deleted file contains a PTY write, and `mailbox.rs` is untouched |
| `session/selection.rs:3838-3899` | recursively reads **every** Rust source for lifecycle-ownership violations | Deleting files cannot add a violation; R7 contains none of the ownership sentinel's event/mutator patterns |
| `tests/cli_workgroup_team.rs:1781-1869` | `commands/entity_creation.rs`, asserts the activation-token count | R7 contains no `ManifestActivationToken::production()` construction, so the count stays at three |
| `testability/ui_automation.rs:2592-2638` | `../src/shared/types.ts`, parses the `UiAutomationAction` union | That union starts at `types.ts:847` and its terminating semicolon precedes the deleted `:981-998`, so the parser reads byte-identical text |
| `pty/watchers/mod.rs:2470`, `commands/session.rs:7432/:7492/:7790/:8012`, `loops/scheduler.rs:644`, `phone/mailbox.rs:11320` | each reads one specific unrelated file | None reads any file this change touches |

There is no test asserting a registered-command count and no desktop/web command-parity test. `ipc.transport.test.ts` dynamically imports the whole IPC module but never enumerates its exports. No JSON fixture or snapshot contains the deleted conversation shape, no Rust doctest names a removed item, and the companion `repo-agentscommander_webpage` contains no removed symbol and no `conversations/` reference.

### 9.2 Verification method, and why it is grep-first

Per §2.4, neither the Rust nor the TypeScript gate can see a leftover `pub` item or an orphaned TS symbol. The build gates prove the tree still compiles; **the greps prove the deletion was complete.** Both are required. Neither substitutes for the other.

### 9.3 The nine verification rules

1. **Run every completeness grep and every line count against the working tree, never against `d7285ce`.** A command of the form `git grep <sym> d7285ce -- <path>` reads the tree *before* any edit and returns the same answer whether the work is complete, half-done, or never started. It cannot fail and it cannot pass. Use `d7285ce` only to re-read the baseline for orientation, never as an acceptance check. Under §8.2 the batch is committed before gating, so at gate time the working tree and `HEAD` are identical and either form is valid; the commands below use the working-tree form.
2. **Pathspec is what protects the greps from this plan file, not the revision.** This file is tracked, so an unscoped `git grep -w PhoneAPI` matches it 22 times. Every per-batch grep below is scoped to `src/`, `src-tauri/`, `docs/`, `PRIVACY.md` or `docs/security.md`, none of which can reach `plans/`. The two repo-wide sweeps in the Final block carry `':!plans/'`, which is the correct and sufficient guard there.
3. **Use `-w` (word-regexp) for every symbol name.** Substring matching produces `sync_workgroup_repos` -> `sync_workgroup_repos_inner` and `SessionGroup` -> `TeamSessionGroup` style false positives.
4. **Scope by pathspec where a name has an English-word homonym, and do not add `-i` to make a grep "stricter".** Specifically: `-w Conversation` over `src-tauri/src/phone/ src/` is correct and safe, because the surviving `phone/mailbox.rs` contains "conversation" on three lines, always lowercase. **Adding `-i` converts criterion 10 into a guaranteed false failure.** The same hazard put `Conversation` prose in `pty/container_paths.rs:245,:592` and `docs/comparison.md:43`, which is why the pathspec exists at all (§6.2).
5. **`git ls-tree` requires a tree-ish and cannot be turned into a working-tree check by dropping the revision.** `git ls-tree -r --name-only -- <path>` exits 128 with `fatal: Not a valid object name`. Worse, `git ls-tree -r --name-only HEAD -- <missing-path>` exits **0** with empty output, so exit status is never proof of absence. **Assert on output, not on exit status**, and use `test ! -e` for the filesystem. Both forms are written out in §9.4. **No acceptance rule may treat a Git usage error as proof of deletion.**
6. **Disambiguate at the definition site.** For the three traps in §6.3, reading the struct body is the check. Match count is not.
7. **A passing build is never evidence that a TS or `pub` Rust symbol was removed.** Only the grep is.
8. **A passing symbol suite is not evidence that the change stayed in scope.** All symbol, count and build criteria can pass while an unrelated tracked file has been edited. The scope gate in the Final block is what closes that, and it is mandatory.
9. **A positive documentation check must be one a wrong sentence fails, it must be anchored to the line it certifies, and for a claim about storage it must anchor the non-exhaustiveness.** Round 1's `docs/security.md` gate grepped for `` `messaging/` ``, `` `outbox/` `` and `_team_`, all three of which the factually false round-1 line satisfied; it certified vocabulary, not truth. Round 2's gate fixed that and still would have certified the round-2 line, which was also inaccurate. Four properties, all required:
   - **(a) At least one anchor must be a phrase the wrong version cannot contain.** For D4 that was the config-directory half round 1 omitted.
   - **(b) Every anchor must resolve to the same line number**, so a token appearing elsewhere in the file cannot stand in for the line under test.
   - **(c) A claim about where data lives must anchor its own non-exhaustiveness.** This is the round-3 addition and it is the one that generalises. Rounds 1 and 2 both failed because the sentence was categorical ("instead", "everything", "in two locations"), and under a universal quantifier every unaudited artifact is a future falsification. Gating on the disclaimer (`neither list is exhaustive`) makes a categorical rewrite fail the gate mechanically, which no vocabulary anchor can do.
   - **(d) A negative probe on every superseded wording**, accumulated rather than replaced. Each round's rejected phrasing stays in the negative set, because the failure mode is drift back toward a claim that reads well.

   Properties (a) and (d) only rule out phrasings already known to be wrong. **(c) is the only one that constrains the next wrong sentence**, which is why it exists.

### 9.4 Verification commands, per batch

All commands are run from the repository root, against the working tree, after that batch has been committed (§8.2). Written for Git Bash / POSIX `sh`, which is available in this environment.

**After batch 1 (TypeScript), owner `dev-webpage-ui`:**

```bash
npm run typecheck                 # must exit 0
npm test                          # must pass

# Completeness. Each must return exit 1 (no output):
git grep -n -w PhoneAPI            -- src/
git grep -n -w PhoneMessage        -- src/
git grep -n -w AgentInfo           -- src/
git grep -n -w syncWorkgroupRepos  -- src/
git grep -n 'phone_'               -- src/
git grep -n 'sync_workgroup_repos' -- src/

# Nothing live was collateral damage. Each must still return hits:
git grep -n -w AcWorkgroup         -- src/shared/types.ts
git grep -n -w WorkgroupGroup      -- src/shared/types.ts
git grep -n -w listAllAgents       -- src/shared/ipc.ts
git grep -n -w AcDiscoveryAPI      -- src/shared/ipc.ts

# Line counts
wc -l < src/shared/ipc.ts     # expect 1235
wc -l < src/shared/types.ts   # expect 1445
```

**After batch 2 (Rust, phone chain), owner `dev-rust`:**

```bash
# Entry precondition: rerun batch 1's six completeness greps. All must be exit 1.
# A hit here means batch 1 did not land, NOT that batch 2 is wrong. See 6.4.

cd src-tauri
cargo check  --all-targets
cargo clippy --all-targets -- -D warnings       # zero warnings
cargo test   --lib --bins --tests
cd ..

# Completeness. Each must return exit 1:
git grep -n -E 'phone_send_message|phone_get_inbox|phone_list_agents|phone_ack_messages' -- src-tauri/
git grep -n -E 'phone::manager|commands::phone|super::manager'                           -- src-tauri/
git grep -n -w Conversation   -- src-tauri/src/phone/ src/       # do NOT add -i, see rule 4
git grep -n -w PhoneMessage   -- src-tauri/ src/

# The two deleted files are gone. Assert on OUTPUT, not exit status (rule 5):
test ! -e src-tauri/src/phone/manager.rs
test ! -e src-tauri/src/commands/phone.rs
test -z "$(git ls-tree -r --name-only HEAD -- src-tauri/src/phone/manager.rs)"
test -z "$(git ls-tree -r --name-only HEAD -- src-tauri/src/commands/phone.rs)"

# AgentInfo: exactly 4 hits, all in entity_creation.rs (:39, :2705, :2706, :2756):
git grep -n -w AgentInfo -- src-tauri/ src/

# The surviving phone/ directory is exactly these five files:
expected_phone_files="$(printf '%s\n' \
  src-tauri/src/phone/consumption.rs \
  src-tauri/src/phone/mailbox.rs \
  src-tauri/src/phone/messaging.rs \
  src-tauri/src/phone/mod.rs \
  src-tauri/src/phone/types.rs)"
actual_phone_files="$(git ls-tree -r --name-only HEAD -- src-tauri/src/phone/)"
test "$actual_phone_files" = "$expected_phone_files"

# Live surface intact. Each must still return hits:
git grep -n -w OutboxMessage       -- src-tauri/src/phone/types.rs
git grep -n -w MESSAGING_DIR_NAME  -- src-tauri/src/phone/messaging.rs
git grep -n -w can_communicate     -- src-tauri/src/config/teams.rs
git grep -n 'pub mod phone;'       -- src-tauri/src/lib.rs

# Line counts
wc -l < src-tauri/src/phone/types.rs    # expect 883
wc -l < src-tauri/src/phone/mod.rs      # expect 4
wc -l < src-tauri/src/commands/mod.rs   # expect 22
```

**After batch 3 (Rust, sync wrapper), owner `dev-rust`:**

```bash
cd src-tauri
cargo check  --all-targets
cargo clippy --all-targets -- -D warnings
cargo test   --lib --bins --tests
cd ..

# sync_workgroup_repos: exactly ONE surviving hit, the log string at :3629.
# Two hits means batch 1 did not land and ipc.ts:1074 is still there. See 6.4.
git grep -n -w sync_workgroup_repos -- src-tauri/ src/

# The inner helper and its live caller survive.
git grep -n -w sync_workgroup_repos_inner -- src-tauri/
    # expect exactly 3 hits after the cut:
    #   :3399  the live call inside update_team
    #   :3449  a code comment referencing it ("...emitted straight to the UI by
    #          `sync_workgroup_repos_inner`..."), out of scope and untouched
    #   :3462  the definition
    # The 4th baseline hit, :3685 inside the deleted wrapper, is gone.
git grep -n -w SyncResult  -- src-tauri/   # expect 3 hits: :91, :3470, :3471
git grep -n -w SyncError   -- src-tauri/   # expect 3 hits, unchanged

# Line counts
wc -l < src-tauri/src/lib.rs                       # expect 3589
wc -l < src-tauri/src/commands/entity_creation.rs  # expect 7793
```

**After batch 4 (docs), owner `technical-writer`:**

```bash
# Completeness. Each must return exit 1:
git grep -n -i -E 'phone|conversations' -- PRIVACY.md
git grep -n -w -E 'PhoneAPI|PhoneMessage|Conversation|AgentInfo|CONVDIR|C_PHONE|PH_MGR|A10|R9' -- docs/reference/architecture.md
git grep -n 'conversations' -- docs/agents/inter-agent-messaging.md docs/security.md

# Kept lines. Each must still return a hit:
git grep -n 'PH\["phone/'    -- docs/reference/architecture.md      # :34
git grep -n 'CMD --> PH'     -- docs/reference/architecture.md      # :53
git grep -n 'PH <-->'        -- docs/reference/architecture.md      # :60

# PRIVACY.md: the guarantee survives literally, and the universal claim is gone.
# Each must return a hit, and all must be on line 31:
git grep -n -F 'entirely local'                       -- PRIVACY.md
git grep -n -F 'No external network calls are made.'  -- PRIVACY.md
git grep -n -F '`messaging/`'                         -- PRIVACY.md
git grep -n -F 'The file-based path'                  -- PRIVACY.md
git grep -n -F 'other delivery paths'                 -- PRIVACY.md

# The superseded universal claim is gone. Must return exit 1:
git grep -n -F 'Messages are written as Markdown files' -- PRIVACY.md

# docs/security.md: the corrected location claim is positively present, it names
# BOTH roots, it names the queue path correctly, and it disclaims its own
# completeness. §9.3 rule 9: the first three greps alone are NOT a gate (the
# false round-1 line satisfied all three), and anchors 4-5 alone are not either
# (the inaccurate round-2 line satisfied those). Anchors 6-8 are what a
# categorical rewrite cannot carry.
# Each must return exactly one hit, and every hit must be line 14:
git grep -n -F '`messaging/`'                                 -- docs/security.md
git grep -n -F 'outbox/'                                      -- docs/security.md
git grep -n -F '_team_'                                       -- docs/security.md
git grep -n -F 'ac-root-agent/'                               -- docs/security.md
git grep -n -F 'instances/<instance-id>/outbox/'              -- docs/security.md
git grep -n -F '<replica>/<instance-dir>/outbox/'             -- docs/security.md
git grep -n -F 'neither list is exhaustive'                   -- docs/security.md
git grep -n -F 'where to start looking'                       -- docs/security.md

# Every superseded wording is gone. Accumulated, not replaced (§9.3 rule 9d).
# Each must return exit 1:
git grep -n -i -F 'project workspace instead'                 -- docs/security.md
git grep -n -F 'Everything is plain files'                    -- docs/security.md
git grep -n -F 'Inter-agent messages live in both locations'  -- docs/security.md
git grep -n -F '`outbox/` delivery queue'                     -- docs/security.md

# One storage model across both documents: each names the Root Agent as a place
# messages live. Each must return a hit. Note that `docs/security.md:42` and `:59`
# already say "Root Agent" for unrelated reasons, which is why the anchor is the
# longer phrase and why the line-14 assertion above is the binding part.
git grep -n -F 'Root Agent directory' -- PRIVACY.md
git grep -n -F 'Root Agent directory' -- docs/security.md

# Line counts
wc -l < docs/reference/architecture.md        # expect 725
wc -l < PRIVACY.md                            # expect 54
wc -l < docs/agents/inter-agent-messaging.md  # expect 214
wc -l < docs/security.md                      # expect 121
```

Mermaid structural check. `architecture.md` holds **15** Mermaid fences and D1 edits **4** diagrams (§2 Rust Backend Modules, §3.3 Shared Layer, §4 IPC Contract, §8 Persistence). No fence is added or removed, so the count must be unchanged:

````bash
grep -c '^```mermaid' docs/reference/architecture.md   # must print 15
````

Together with the node-id grep above, that is the **binding** Mermaid gate: every removed id is gone from the file, and no fence was damaged. Opening the file in a Mermaid preview is **non-binding manual QA**, explicitly optional. It is not an acceptance criterion, because the repository ships no `mmdc` or documentation-render script (`package.json` has the `mermaid` runtime library only), and adding one would be a new dependency, which §3.3 rules out.

**Final, across the whole change. Run by batch 4's owner:**

```bash
# 1. Symbol sweep, repo-wide. Must return exit 1.
git grep -n -i -E 'phone_send_message|phone_get_inbox|phone_list_agents|phone_ack_messages|PhoneAPI|syncWorkgroupRepos' -- . ':!plans/'

# 2. Full build and test suite.
npm run typecheck && npm test && npm run test:debt
cd src-tauri && cargo check --all-targets \
  && cargo clippy --all-targets -- -D warnings \
  && cargo test --lib --bins --tests
cd ..

# 3. SCOPE GATE. Mandatory. Clean tree, no whitespace damage, and an exact path whitelist.
git status --porcelain                                                              # must be empty
git diff --check       d7285ce..HEAD -- . ':!plans/1179-remove-dead-phone-and-sync-repos.md'
git diff --name-status d7285ce..HEAD -- . ':!plans/1179-remove-dead-phone-and-sync-repos.md'
```

The `--name-status` output must be **exactly** these 13 lines, and nothing else:

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

The whitelist catches an unrelated file. The line counts in the per-batch blocks catch an extra edit inside a whitelisted file. **Read the production diff against §5's exact cuts** as the last step: that catches same-file reformatting that happens to preserve the line count.

### 9.5 Objective acceptance criteria

Every criterion is decidable by running a listed command and comparing the result. No judgement is required. Criteria 7 onwards are evaluated against the **working tree**, never against `d7285ce`.

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
| 10 | `Conversation` returns **zero** hits under `src-tauri/src/phone/` and `src/`, case-sensitive | batch 2 grep |
| 11 | `AgentInfo` returns **exactly 4** hits under `src-tauri/` and `src/`, all in `commands/entity_creation.rs` (`:39`, `:2705`, `:2706`, `:2756`) | batch 2 grep |
| 12 | `sync_workgroup_repos` (word-regexp) returns **exactly 1** hit under `src-tauri/` and `src/`: the log string in the inner helper | batch 3 grep |
| 13 | `syncWorkgroupRepos` returns **zero** hits under `src/` | batch 1 grep |
| 14 | `sync_workgroup_repos_inner` returns **exactly 3** hits under `src-tauri/`: the live call at `:3399`, the code comment at `:3449`, and the definition at `:3462` | batch 3 grep |
| 15 | `SyncResult` returns 3 hits, `SyncError` returns 3 hits, both in `entity_creation.rs` | batch 3 grep |
| 16 | `src-tauri/src/phone/` at `HEAD` contains **exactly** `consumption.rs`, `mailbox.rs`, `messaging.rs`, `mod.rs`, `types.rs`, and the two deleted files are absent from both the filesystem and `HEAD` | batch 2 `test`/`ls-tree` assertions |
| 17 | `phone/types.rs` still exports `OutboxMessage` and the `PtyInput*` surface; `lib.rs:11 pub mod phone;` is present | batch 2 greps |
| 18 | `docs/reference/architecture.md:34`, `:53` and `:60` are present | batch 4 greps |
| 19 | No removed symbol is documented anywhere: the architecture.md symbol grep returns exit 1 | batch 4 grep |
| 20 | `PRIVACY.md` contains the literals `No external network calls are made.`, `entirely local`, `` `messaging/` ``, `The file-based path` and `other delivery paths`, **all on line 31**; contains **zero** `phone` or `conversations` hits; and the superseded universal `Messages are written as Markdown files` returns **zero** hits | batch 4 greps |
| 21 | `docs/agents/inter-agent-messaging.md` and `docs/security.md` contain **zero** `conversations` hits; `docs/security.md` positively contains all eight D4 anchors (`` `messaging/` ``, `outbox/`, `_team_`, `ac-root-agent/`, `instances/<instance-id>/outbox/`, `<replica>/<instance-dir>/outbox/`, `neither list is exhaustive`, `where to start looking`), **each returning exactly one hit and each on line 14**; and all four superseded phrasings (`project workspace instead`, `Everything is plain files`, `Inter-agent messages live in both locations`, `` `outbox/` delivery queue ``) return **zero** hits | batch 4 greps |
| 22 | Every one of the 13 line counts in the §1.3 table matches, read from the working tree | per-batch `wc -l` checks |
| 23 | `architecture.md` still holds exactly 15 Mermaid fences | batch 4 structural check |
| 24 | **Scope gate.** `git status --porcelain` is empty, `git diff --check` is clean, and `git diff --name-status d7285ce..HEAD` excluding this plan file lists **exactly** the 13 paths and statuses in §9.4, and nothing else | final scope gate |
| 25 | `CHANGELOG.md` is unmodified | implied by criterion 24; `CHANGELOG.md` is not on the whitelist |
| 26 | The branch contains the 4 implementation commits of §8.5, in that order and with those messages. Any `docs(#1179)` commit carrying this plan file is separate and does not count against this criterion (§9.6). The round-2 follow-on batch adds one further implementation commit and does not invalidate this criterion (§8.7) | `git log --oneline d7285ce..HEAD` |
| 27 | **One storage model across both documents.** `PRIVACY.md` and `docs/security.md` each contain the literal `Root Agent directory`, so neither describes message storage as exclusive to the other's location; and **neither document states a universal.** The four negative probes of criteria 20 and 21 are the mechanical form of the second half | batch 4 greps |

### 9.6 Note on this plan file and `.gitignore`

`.gitignore:11` ignores `plans/`, so this file is **untracked by default** and reaches the repository only via `git add -f`. Six plan files are in the repository that way (`plans/1038-*`, `1057-*`, `1070-*`, `1072-*`, `1171-*`, `1177-*`). Precedent from the immediately preceding change is two dedicated commits: `092d85c docs(#1177): add implementation plan for dead-code removal`, then `c93bff0 docs(#1177): certify plan READY_FOR_IMPLEMENTATION (Step 7 consensus)`.

Three consequences:

1. **Committing this plan is not an implementer's job** and is not one of the four batches. It belongs to the plan-authoring and certification workflow.
2. **This file is tracked once committed, so `git grep` does see it.** An unscoped `git grep -w PhoneAPI` matches it 22 times. That is exactly why §9.3 rule 2 makes the pathspec, not the revision, the protection: every per-batch grep is scoped to a directory that cannot reach `plans/`, and the two repo-wide sweeps carry `':!plans/'`.
3. **The scope gate excludes only this plan file**, by exact path. `plans/1177-remove-dead-code.md` is tracked and unmodified by this change, so it never appears in the diff.

---

## 10. Notes for the batch owners

**Read this section before starting your batch.** You are working from a purged context: everything you need is in this file, and you do not need the issue body, the revalidation reports, or any prior conversation.

### 10.1 Which parts are yours

| You are | Your batch | Read in full | Skim for context |
| --- | --- | --- | --- |
| `dev-webpage-ui` | 1 | §5.0, §5.3, §6.3 (the TS `AgentInfo` trap), §8, §9.3, §9.4 batch 1 | §1, §2.4, §3 |
| `dev-rust` | 2 and 3 | §5.0, §5.1, §5.2, §6.3, §6.4, §8, §9.3, §9.4 batches 2 and 3 | §1, §2, §3 |
| `technical-writer` | 4 | §4.2, §4.3, §4.6, §5.4, §8, §9.3, §9.4 batch 4 | §1, §2.5, §3 |
| `technical-writer` | 5 (round 3) | §4.2, §4.6, §4.6.1, §8.7, §9.3 rule 9, §9.4 batch 4 | §7 |

Everyone reads §8 in full. It carries the entry precondition, the commit-before-gate rule, and the handoff rule, and those are what keep three owners from corrupting each other's work.

### 10.2 Rules for every owner

1. **The issue body contains errors this plan has already corrected.** If you read it and it disagrees with §5, **§5 wins**. The disagreements are listed in §12 so you can tell a correction from a mistake.
2. **Do not start until your entry precondition passes** (§8.1). Rerunning the previous batch's greps takes seconds and is the only thing standing between you and building on unverified work.
3. **Commit your batch before you gate it, and hand off only after the gate passes in full** (§8.2, §8.3). A commit is not evidence a gate passed. If you inherit a commit whose gate you did not watch complete, rerun the whole gate on that `HEAD`.
4. **Run verification greps against the working tree, never against `d7285ce`.** A `d7285ce`-pinned grep reads the tree before your edits and will report the symbols you just deleted as still present. §9.3 rule 1.
5. **`git ls-tree` needs a tree-ish, and its exit status never proves absence.** Use the `test ! -e` and `test -z "$(...)"` forms in §9.4 exactly as written. §9.3 rule 5.
6. **Do not "improve" a grep by adding `-i`.** It converts criterion 10 into a guaranteed false failure. §9.3 rule 4.
7. **A green build does not prove a deletion was complete.** This repo has no `noUnusedLocals` and no linter, and rustc emits no `dead_code` for `pub` items in a `lib` crate. The greps are the real gate. §2.4.
8. **Do not reformat anything, and touch only the 13 files in the §1.3 table.** The scope gate will catch you.
9. If a gate fails in a way §6.4 does not describe, **stop and report it.** That is new information, not something to work around.

### 10.3 Batch-specific traps

**Batch 1 (`dev-webpage-ui`):**
- Apply the three `ipc.ts` cuts **bottom-up**: `:1071-1075`, then `:806-815`, then `:28-29`. §5.0 rule A.
- `syncWorkgroupRepos` orphans **no** import; its return type is inline and anonymous. Do not go hunting for one. §5.3 T4.
- `PhoneConversation` is already gone. `types.ts:1041-1046` now holds live `AcWorkgroup`/`WorkgroupGroup` code. **Do not go near those lines.** §5.3 T3.
- TS `AgentInfo` is not the type `EntityAPI.listAllAgents` uses; that one is inlined. §6.3.

**Batch 2 (`dev-rust`):**
- **Batch 2 is atomic.** Apply all six edits, then gate. A `cargo check` between them will fail and the failure means nothing. §4.1.
- **`phone/types.rs` is not a phone-types file.** It is 913 lines of live PTY-input protocol with 30 lines of phone types appended at the end. You remove `:779-808` and nothing else. Do not open it expecting to delete it.
- **Keep `types.rs:4` `use serde::{Deserialize, Serialize};`.** `Deserialize` and `Serialize` each appear 12 times in the surviving `:1-777`. Removing it is a compile error, not a cleanup.
- **The live surface under `phone/` is everything except `manager.rs`.** `consumption.rs`, `mailbox.rs` (21,707 lines, the real CLI messaging system), `messaging.rs`, `mod.rs`, `types.rs` all stay. **Do not touch `lib.rs:11 pub mod phone;`.**
- Your first gate is a **cold build**. Budget for it and do not read it as a hang. §8.4.
- Three homonym traps (`AgentInfo`, `send_message`, TS `AgentInfo`) plus one near-homonym (`sync_workgroup_repos` vs `..._inner`). Read the definition site; never trust a match count. §6.3.

**Batch 3 (`dev-rust`):**
- **After batch 2, `lib.rs:2667` has become `:2663`.** Locate R8 by anchor text, not by line number. §5.0 rule B.
- **`plans/1177-remove-dead-code.md:625` records the sync wrapper at `:3678`. That is stale.** The real location is `:3666`; the stale number lands you 12 lines inside the function body.
- Do not touch `sync_workgroup_repos_inner`, `SyncResult`, `SyncError`, or the `:3629` log string. §5.2.
- Between batch 2 and batch 3, **do not disturb `src-tauri/target/`**: no `cargo clean`, no toolchain switch, no different `CARGO_TARGET_DIR`. Otherwise batch 3's cheap incremental gate becomes a second cold build. §8.4.

**Batch 4 (`technical-writer`):**
- Apply D1's 17 edits **bottom-up**, highest line number first. §5.0 rule A gives the exact order.
- **`:623 style CONVDIR` must go with `:611`.** Deleting only the node leaves Mermaid styling an undeclared id. §5.4.
- **`:34`, `:53` and `:60` must STAY.** They describe the `phone/` directory, which survives. §3.2.
- D2, D3 and D4 have exact replacement text in §4.2, §4.3 and §4.6. **Use it verbatim.** These are decided wordings with per-clause evidence, not drafts. D4 in particular is a **rewrite of the whole enumeration item**, not a token deletion; §4.6 explains why.
- You run the **Final** block, including the scope gate, because you are last. §8.5.

**Batch 5 (`technical-writer`, round 3):**
- **The text already in the tree is wrong in both files.** `docs/security.md:14` and `PRIVACY.md:29-31` both carry superseded wording that reads plausibly. Replace each block in full with §4.6's and §4.2's round-3 text; do not diff by eye and patch the difference.
- **§4.6 has three superseded versions on record and §4.2 has two.** Only the block labelled "Replacement" ships. If you are looking at a version that says "instead", "Everything is plain files", or "Messages are written as Markdown files", you are reading a superseded one.
- **The clauses that matter most are the weak ones.** "you will find", "neither list is exhaustive", "The file-based path", "other delivery paths". They look like hedging and they are the entire point: three rounds failed because the text was categorical. Do not tighten them, do not make them read more confidently, and do not "improve" the grammar in a way that restores a universal.
- Your gate anchors the non-exhaustiveness clause, and every `docs/security.md` anchor must land on line 14. §9.3 rule 9.
- Two files, four lines total. The scope gate still expects the same 13 paths. §8.7.

---

## 11. Decisions (all closed)

| # | Decision | Resolution | Where |
| ---: | --- | --- | --- |
| 1 | Batching and commit order | 4 batches, 4 commits: TS -> Rust phone -> Rust sync -> docs. Batch 2 is atomic. | §4.1 |
| 1a | Ownership and execution order | **Three owners, strictly sequential**: batch 1 `dev-webpage-ui`, batches 2 and 3 `dev-rust`, batch 4 `technical-writer`. One owner in the tree at a time. Order is a **hard constraint**, not a preference. | §8.0 |
| 1b | Cross-tree gates vs. narrowed per-language gates | **Gates stay cross-tree; the order carries the constraint.** Narrowing was considered and rejected: strict sequencing is already forced by the single clone, and narrowing would cost the cumulative whole-repository property of criteria 9, 11 and 12. | §8.0 |
| 1c | Commit before gating, or gate before committing | **Commit first, then gate**, amending or squashing into the batch commit on failure. Gating first leaves the tree unprotected for exactly the interval the per-batch commit rule exists to cover. | §8.2 |
| 1d | What counts as a handoff signal | **An explicit gate-pass report, never the presence of the commit.** After a session loss the recovering owner reruns the whole gate on that `HEAD`. | §8.3 |
| 1e | Revision that acceptance runs against | **The working tree, never `d7285ce`.** A baseline-pinned acceptance suite verifies the state the change removes and can neither pass nor fail. Protection against this plan file is the **pathspec**, not the revision. | §9.3 rules 1-2 |
| 1f | `git ls-tree` in acceptance | **Never as an exit-status check.** It requires a tree-ish, and `ls-tree HEAD -- <missing>` exits 0. Replaced with `test ! -e` plus `test -z "$(...)"` output assertions and an exact surviving-directory comparison. | §9.3 rule 5, §9.4 |
| 1g | Scope enforcement | **A mandatory final scope gate**: clean `git status`, `git diff --check`, and an exact 13-path `git diff --name-status` whitelist. The symbol suite alone cannot detect an unrelated edit. | §9.3 rule 8, §9.4 Final |
| 1h | Mermaid acceptance | **Render preview is non-binding manual QA.** The binding gate is the node-id grep plus a fence-count check (15 fences, unchanged). D1 edits **4** diagrams, not 5. No render dependency is added; §3.3 rules that out. | §9.4 batch 4 |
| 2 | Exact `PRIVACY.md` replacement wording | Specified verbatim, 3 lines for 3 lines, with a per-clause evidence table. **Rewritten at round 3**: the shipped text universally claims every message is a Markdown file in `messaging/`, which the control-plane API path falsifies. The privacy guarantee is preserved verbatim; the universal framing is not. | §4.2 |
| 3 | `docs/agents/inter-agent-messaging.md:129-132` | **Delete** the `## Conversation files` section (no guarantee to preserve, unlike `PRIVACY.md`) | §4.3 |
| 4 | `docs/security.md:14` | **Rewrite the whole enumeration item**, not a token deletion. Dropping `conversations, ` alone leaves two verified falsehoods (teams and messages both claimed to live under `~/.agentscommander/`). Exact wording specified. | §4.6 |
| 4a | Which location the rewritten `docs/security.md:14` names for messages | **Both, and it says its list is not closed.** Round 1 said the project workspace "instead", false for Root Agent `messaging/`. Round 2 said both but categorically, and validation falsified the categorical with a fifth store. Round 3 keeps both roots, drops every universal quantifier, and disclaims exhaustiveness in the line itself. Ships as batch 5. | §4.6, §4.6.1, §8.7 |
| 4b | Whether `PRIVACY.md`'s §4.2 wording changes | **Not at round 2; yes at round 3.** Rounds 1 and 2 both judged the shipped text correct. It is not: it universally claims every message becomes a Markdown file in `messaging/`, and the control-plane API path creates no file. The guarantee clauses are preserved verbatim; the universal framing is replaced by a path-scoped description. | §4.2 |
| 4c | What makes a positive documentation gate binding | **An anchor the wrong sentence cannot carry, a line-number assertion, an anchor on the non-exhaustiveness clause, and an accumulated negative set.** Round 1's gate certified vocabulary; round 2's would have certified the categorical round-2 line. Only the non-exhaustiveness anchor constrains the *next* wrong sentence. §9.3 rule 9, properties (a) through (d). | §9.3 rule 9, §9.4 batch 4 |
| 4d | Whether to name the API message database and `send --outbox` in these two documents | **No, by the user's scope decision relayed at round 3.** Both are real and both are verified in §4.6.1's evidence, but naming them re-enters the exhaustiveness trap: the next unaudited store falsifies the sentence again. The texts are instead written so that neither can contradict them. The full storage audit is issue **#1195**. | §4.6.1, §12.2 item 12 |
| 4e | What the two documents claim after batch 5 | **True, mutually consistent, and explicitly incomplete.** This is a deliberate reduction in what the documents assert, not an oversight; §7 states it plainly rather than implying completeness. | §7, §4.6 |
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
| 16 | Committing this plan file, given that `.gitignore:11` ignores `plans/` | **Not a batch owner's job and not one of the four batches.** It needs `git add -f` and belongs to the authoring/certification workflow, matching the #1177 precedent. | §9.6 |

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
| 11 | Not addressed. Added at Step 7 round 1 after `dev-rust-grinch` (G3) challenged D4; **corrected at Step 7 round 2** after the Step 9 review found the round-1 replacement itself false | **`docs/security.md:14` carries two more false location claims, not one.** Removing the `conversations` token leaves the line asserting that **teams** and **messages** live under `~/.agentscommander/`. Team configuration does not live there at all (`<workspace>/_team_<name>/`; no code constructs a `teams.json` path anywhere, the one occurrence of that name being CLI help prose at `cli/send.rs:22`). Messages live there **in part**: workgroup `messaging/` and replica `outbox/` are in the project workspace, while Root Agent `messaging/` (`<config-dir>/ac-root-agent/messaging/`) and the instance app outbox (`<config-dir>/instances/<instance-id>/outbox/`) are under the config directory. §4.6 rewrites the whole item to name both roots. | A security document telling a reader the wrong place to secure, back up, or erase their data. **Round 1 half-corrected it**: it moved messages wholesale into the workspace, which is the mirror-image error and would have left Root Agent messages and the instance outbox undocumented. It would also have left `PRIVACY.md` and `docs/security.md` describing two different storage models. |

| 12 | Not addressed. Added at Step 7 round 3 | **`PRIVACY.md:29-31` is also false as shipped, and the fix is to stop quantifying rather than to enumerate.** The line claims every inter-agent message is written as a Markdown file into a `messaging/` directory. The opt-in control-plane API stores bodies inline in `<config-dir>/api-message-bus.sqlite3` and dispatches from that queue with no Markdown file created. §4.2 rewrites the sentence to describe the file-based path as one path. | A privacy policy making a universal claim about message storage that one live delivery path already contradicts. Three rounds of this change asserted it or left it standing. |
| 13 | Not addressed. Added at Step 7 round 3 | **Neither document can carry a closed inventory, and both now say so.** Three successive rewrites were each falsified by an artifact the previous one had not audited: the Root Agent `messaging/` directory, the instance app outbox, the API message database, the caller-supplied `--outbox`. §4.6 and §4.2 are therefore bounded and non-categorical by design, and §9.3 rule 9(c) gates the disclaimer so a categorical rewrite fails mechanically. The complete storage audit is **#1195**. | Without this, round 4 would have found a sixth store and the cycle would repeat. The user's scope decision was to bound the claim rather than chase completeness inside a dead-code-removal change. |

### 12.3 No reason was found to stop

The change should proceed as specified. Every "dead" claim was re-verified independently against `d7285ce` by the author and again by `dev-rust` at Step 5; every one holds. Neither reviewer found a missing production deletion, a seventh batch-2 edit, a hidden caller, or a platform or resource blocker. The eleven additions in §12.2 widen the documentation scope by roughly 6 lines across 2 files and correct one Mermaid rendering defect; they do not change the shape of the work or its risk profile.

Every defect found at Steps 5 and 6 was in the **verification apparatus**, not in the edits: what revision acceptance ran against, whether `git ls-tree` can prove absence, whether the batches could be reordered, whether the scope was bounded, and whether a documentation claim was true. All are resolved in §13.

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

### Architect resolution and certification (Step 7, round 1)

**Verdict: `READY_FOR_IMPLEMENTATION`.** All fourteen enrichment items are resolved. Nothing was deferred to a second round, and no decision was left open.

**Both reviewers' cold-start verdict was "no as written, yes once these are resolved," and they were right.** E1 is a genuine defect of mine: §9.3 rule 1 pinned every acceptance grep and line count to `d7285ce`, so the suite verified the state the change removes and returned the same answer whether the work was complete, half-done, or never started. The coordinator supplied that pin for **revalidation**, where it is correct; carrying it into **acceptance**, where the revision is inverted, was my error. Sixteen of twenty-four criteria were affected and the failure mode was indistinguishable from broken work. §9 has been rewritten rather than patched.

**Every command G1 proposed was re-derived against this repository before adoption, not trusted.** Two consecutive fixes in this area had already shipped bugs, so the same skepticism was applied to the third. Results, run on this branch:

````text
$ git ls-tree -r --name-only -- src-tauri/src/phone/
fatal: Not a valid object name src-tauri/src/phone/          exit=128     <- G1 confirmed
$ git ls-tree -r --name-only HEAD -- does/not/exist
                                                              exit=0       <- G1 confirmed
$ expected="$(printf '%s\n' ...5 paths...)"; actual="$(git ls-tree -r --name-only HEAD -- src-tauri/src/phone/)"
$ test "$actual" = "$expected"                                             <- mechanism verified working
$ git diff --name-status d7285ce..HEAD -- . ':!plans/1179-...md'
                                                              (empty)      <- G4 scope gate verified working
$ grep -c '^```mermaid' docs/reference/architecture.md
15                                                                         <- G6 confirmed
$ git grep -c -w PhoneAPI -- .          -> includes plans/1179-...md:22    <- confirms rule 2's premise
$ git grep -c -w PhoneAPI -- src/ src-tauri/ docs/   -> plan file absent   <- confirms pathspec is sufficient
````

#### Disposition of every item

| Item | Disposition | Resolution |
| --- | --- | --- |
| **E1** | **ADOPTED, with the fix reworked** | §9.3 rule 1 now mandates the working tree and explains why a baseline-pinned suite can neither pass nor fail. Every `git grep` in §9.4 lost its revision; every `git show <rev>:<path> \| wc -l` became `wc -l < <path>` (equivalence verified for all 11 files). E1's instruction to do the same to `git ls-tree` was **not** applied, see G1. §6.2, §6.4 and §9.6 reworded so the stated protection is the pathspec, not the revision. New §6.4 row gives the exact symptom so an implementer who hits a pinned grep diagnoses it in seconds instead of reverting correct work. |
| **E2** | **ADOPTED as evidence** | Batch-2 atomicity confirmed; six edits, no seventh. The source-introspection test class was the genuinely new contribution and is now recorded in §9.1 as a table, merged with G7's three additions. |
| **E3** | **ADOPTED as evidence** | All 13 line counts and every cut boundary independently re-derived and confirmed. §1.3 stays as written and is now readable as acceptance, since E1's fix makes the counts come from the working tree. |
| **E4** | **ADOPTED** | The "do not add `-i` to the `Conversation` grep" hazard is promoted to §9.3 rule 4 with the reason, and to a §6.4 row. This is a real trap: `phone/mailbox.rs` carries lowercase "conversation" on three lines, so `-i` would turn criterion 10 into a guaranteed false failure. E4's request that the two `ls-tree` lines carry a stated expectation is satisfied by G1's replacement. |
| **E5** | **ADOPTED** | Commit-before-gate is now §8.2, and §4.1's rationale, §8.5 and §10 agree with it. E5 correctly caught that §4.1 justified per-batch commits by session-loss risk while §8 and §10.11 told the implementer to commit *after* the gate, leaving the tree exposed for exactly that interval. The cold-build warning and the "do not disturb `src-tauri/target/`" condition are §8.4. |
| **E6** | **ADJUSTED. Its premise is overturned; its request is granted** | E6 asked for per-batch owners: granted, in §8.0 and §8.5, matching the coordinator's split. E6 also concluded that batches 2 and 3 may start before batch 1 lands. **Rejected**, because G2 is right: the source edits are compile-independent but the *gates* are not. §8.0 records both the corrected reasoning and the two concrete failure signatures. |
| **E7** | **ADOPTED** | `:3664-3696` (33 lines) confirmed as the correct wrapper cut against the dispatch brief's 32-line paraphrase. Criterion 22 is downstream of E1 and is fixed by it. The `architecture.md:230` and T4 shape confirmations are recorded. |
| **G1** | **ADOPTED, after independent verification** | Both claims reproduce exactly (output above). §9.3 rule 5 now states that `ls-tree` requires a tree-ish and that its exit status never proves absence, and §9.4 uses `test ! -e` plus `test -z "$(...)"` plus an exact five-path directory comparison. Two §6.4 rows cover both misuse signatures. **The general rule that no acceptance check may treat a Git usage error as proof is stated explicitly**, because that is the class of error, not the instance. |
| **G2** | **ADOPTED as a hard constraint** | §8.0 states the strict order 1 -> 2 -> 3 -> 4 as mandatory with **two independent sufficient reasons**: the single shared clone, and the cross-tree gates. The coordinator asked for one choice between "mandatory order" and "re-scope the greps": **the order carries the constraint and the gates stay cross-tree.** The narrowing alternative is recorded as considered and rejected, with the reason, so it is not reopened. |
| **G3** | **ADOPTED, and widened** | G3 is right that D4 as written still placed messages under `~/.agentscommander/`. Verifying it surfaced more: **the surviving line also claims teams live there, and that is false too** (`<workspace>/_team_<name>/`; there is no `teams.json` anywhere in the codebase). Correcting one falsehood while re-publishing another, in a security document, is not defensible, so §4.6 rewrites the whole enumeration item with a per-clause evidence table, matching §4.2's treatment of `PRIVACY.md`. The widening is flagged in §4.6 and §12.2 item 11 rather than buried. G3's positive location check is §9.4 batch 4 and criterion 21; the literal `No external network calls are made.` assertion is criterion 20. |
| **G4** | **ADOPTED, with additions** | The scope gate is §9.4 Final step 3 and criterion 24, with G4's exact 13-path whitelist (cross-checked against §1.3: identical). Verified the commands run correctly on this branch. Added beyond G4: §9.3 rule 8 states the principle, §8.6 rule 5 states the obligation, and a §6.4 row tells an owner what a 14th path means. Criterion 25 now derives `CHANGELOG.md` from the whitelist instead of asserting it separately. |
| **G5** | **ADOPTED** | §8.3 states the three rules verbatim in effect: no handoff without an explicit gate-pass report, rerun the whole gate on `HEAD` after a session loss, and amend or squash before handoff after a failure. §8.1 adds the matching **entry precondition**, so the receiving owner independently verifies the previous batch rather than trusting a commit. G5 is right that this matters more because the batches cross agents. |
| **G6** | **ADOPTED** | Count corrected: `architecture.md` holds **15** Mermaid fences and D1 edits **4** diagrams (§2, §3.3, §4, §8), not five. Verified. The render preview is now explicitly **non-binding manual QA**. The binding gate is the node-id grep plus a fence-count check (criterion 23). No render dependency is added: the repository ships only the `mermaid` runtime library, and adding `mmdc` would need a separate scope decision that §3.3 forecloses. |
| **G7** | **ADOPTED as evidence** | The three additional source readers (`session/selection.rs`, `tests/cli_workgroup_team.rs`, `testability/ui_automation.rs`) are merged into §9.1's table with G7's reasoning, alongside the negative results for dynamic IPC tests, capability tests, fixtures, snapshots, doctests and the companion webpage repo. This closes the hidden-coupling audit. |
| **G's public-API caveat** | **ADOPTED** | §7 now claims "no supported or known external consumer" and states explicitly that this is the strongest claim the evidence supports, rather than asserting that no private dependent exists anywhere. |

#### Nothing rejected, one premise overturned

No item was rejected. **E6's conclusion that batches 2 and 3 may proceed independently of batch 1 is the single overturned premise**, and it was overturned by G2 on evidence, not by preference. E6's underlying request, per-batch owners, was granted in full. `dev-rust` and `dev-rust-grinch` reached opposite conclusions on that one point; the record above shows which won and why, so the dissent is visible rather than smoothed over.

#### What changed in the plan

Rewritten: §8 (now owners and handoff), §9.3, §9.4, §9.5, §10 (now addressed to three batch owners rather than one implementer).
New: §4.6 (`docs/security.md` wording), §8.0-§8.4, §8.6, §9.3 rules 5 and 8, §9.4 Final scope gate, criteria 23-26.
Amended: header, §4.1, §3.1 D4, §5.4 D4, §6.2, §6.4 (eight new rows), §7, §11 (decisions 1a-1h), §12.2 item 11, §12.3.
Unchanged: §1, §2, §3.2, §3.3, §5.0-§5.3, §5.4 D1-D3, §6.1, §6.3, §9.1's opening, §9.2, §9.6's substance, §12.1. Every cut coordinate in §5 survived two independent re-derivations and was not touched.

#### Certification (round 1, superseded)

This plan was certified `READY_FOR_IMPLEMENTATION` as of Step 7 round 1, digest `FD9E52B772DAAC360DA773614F957596F21BED0C44778A7B1E18422D12577AD4`. **That certification was withdrawn at Step 7 round 2**, for the reason in §13.4.

---

### 13.4 Step 7 round 2: the §4.6 defect and its resolution

Batches 1 through 4 were implemented and reviewed. `dev-rust-grinch` returned **FAIL** on the Step 9 review with a single finding, and the finding is in **this plan's text**, not in the implementation: every R1-R9, T1-T4 and D1-D4 cut matched §5 exactly, all 26 criteria passed mechanically, the full binding suite was green, and the tree was clean at `8c1ea67`. Because a frozen plan cannot be fixed against its own certified artifact, READY was invalidated and the task returned to planning.

#### The finding

§4.6's round-1 replacement for `docs/security.md:14` said team configuration and inter-agent messages live in the project workspace **"instead"**. That is false for Root Agent messaging, and the enumeration omitted the instance app outbox entirely.

#### Evidence, re-derived at round 2 rather than accepted

I did not take the review report as evidence. Every coordinate below was read out of the working tree at `8c1ea67`.

| Claim | Verified at | What it shows |
| --- | --- | --- |
| The Root Agent directory is under the config directory | `config/root_agent.rs:626-628` | `display_path(&config_dir.join(ROOT_AGENT_DIR_NAME))`, with `ROOT_AGENT_DIR_NAME = "ac-root-agent"` at `:13`. Not a workspace path. |
| Its `messaging/` is under that config-dir-owned root | `config/root_agent.rs:722` | `root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME)`, created by `ensure_root_agent_dir_at`. |
| A Root Agent sender writes Markdown messages there | `cli/send.rs:943-946` | `--send` is routed through `root_messaging_dir(agent_root_path)` when `root_is_root_agent`; the workspace branch at `:953-973` is the other half of the same `if`. |
| The instance app outbox is under the config directory | `lib.rs:968-969`, `:996-998` | `config_dir.join("instances")`, then `instances_dir.join(&instance_id).join("outbox")`, created at boot. |
| Root/master-token sends target it | `cli/send.rs:1080-1088` | Reads `app-outbox-path.txt` from `config_dir()` and writes there, falling back to `ac_dir.join("outbox")`. |
| The Root Agent root also holds an `outbox/` | `config/root_agent.rs:709` -> `entity_creation.rs:142-147`, test `:4483`; `cli/send.rs:827`, `:1091` | Found while re-deriving, beyond the review's list. The provisioned layout includes `outbox/`, and a non-root-token send from that root writes `<root>/<local-dir>/outbox/`. Both sit under the config directory, so the round-1 line missed a third artifact, not two. |
| The two documents disagreed | `PRIVACY.md:31` vs `docs/security.md:14` at `8c1ea67` | `PRIVACY.md` says messages live in `messaging/` directories "inside each workgroup **and inside the Root Agent directory**". `docs/security.md` said the workspace "instead". Both were shipped by the same change. |

**The finding is confirmed in full.** The round-1 widening from a token deletion to a rewrite was correct; the replacement text then over-corrected, asserting the mirror image of the falsehood it removed.

#### Root cause, and where it was already visible

The false claim did not originate in §4.6. It originated in **§4.2's evidence table**, whose "No `~/.agentscommander/` path" row read: *"Messaging files do not live under the config directory; they live in the project workspace."* §4.6 was written to be consistent with §4.2 and inherited that sentence as a premise. `PRIVACY.md`'s shipped wording never contained the error, because it describes messages by artifact shape rather than by root, so the error stayed invisible in the text and lived only in the justification. **A wrong reason under a right conclusion survived review once and then propagated into a document where it mattered.** That row is rewritten at round 2 for exactly this reason, even though the sentence it supports is unchanged.

The second-order cause is the gate. §9.4's round-1 positive check grepped `docs/security.md` for `` `messaging/` ``, `` `outbox/` `` and `_team_`, all three of which the false line contained. The gate certified that the line used the right vocabulary, never that it made a true claim. That is now §9.3 rule 9.

#### What changed at round 2

Rewritten: §4.6 (replacement text, both superseded versions shown, per-clause evidence table re-derived from the tree), §4.2's evidence table row plus a re-verification note, §9.4's batch-4 `docs/security.md` block, §12.2 item 11.
New: §8.7 (batch 5, the one-line follow-on), §9.3 rule 9, criterion 27, §11 decisions 4a-4c, §10.3's batch-5 traps, §13.4.
Amended: header, §5.4 D4, §7 (two paragraphs), §9.5 criteria 21 and 26 and its preamble, §9.3's title, §10.1 (adds the batch-5 row; also corrects that row's `§4.4` cross-reference to `§4.6`, since §4.4 is `dev-rust`'s R9 and was never batch 4's).
Unchanged: **`PRIVACY.md`'s replacement text in §4.2**, every §5 cut coordinate, every batch-1 through batch-4 instruction, §1.3's line counts, the 13-path scope whitelist, §3, §6, §8.0-§8.6. No code decision, cut boundary, or batch structure was reopened. `docs/security.md` stays at 121 lines, so no line count moved.

#### One thing I could not close from the plan side

The corrected line is specified but not yet implemented. Batch 5 (§8.7) is the whole of the remaining work, and it is one line in one file.

#### Certification (round 2, superseded)

Certified at Step 7 round 2, digest `C76B945165A30A32315177C7E4D55D7C4E063FC19ED6449D4BBCE6445A5DCA18`. **Withdrawn at Step 7 round 3**, for the reasons in §13.5.

---

### 13.5 Step 7 round 3: validation rejected the round-2 wording, and the standard changed

`dev-rust-grinch` validated the round-2 §4.6 text before implementation and **REJECTED** it with three findings. I re-derived all three from the tree rather than accepting the report. **All three hold.**

#### Finding 1: a fifth message store, unaudited by any round

`<config-dir>/api-message-bus.sqlite3`. Verified: `api/message_store.rs:15` `DB_FILENAME`, `:722-724` `at_config_dir()` opening `config_dir().join(DB_FILENAME)`, `:785-792` the `messages` table with `body TEXT NOT NULL`. `api/dispatcher.rs:180-190` delivers via `WakeDeliveryOrigin::DbQueue` and `:331-337` builds the `OutboxMessage` from `row.body`, so **that path creates no Markdown file at any point**. `api/README.md:3-7` describes the API as speaking the control plane "instead of the filesystem outbox"; `:150-157` calls the database plaintext and sensitive, holding queued message bodies and replayable PTY-input text, and warns that WAL and storage media can retain historical body bytes.

**Consequence beyond §4.6: the shipped `PRIVACY.md:31` is false**, because it universally claims messages are written as Markdown files. Rounds 1 and 2 both examined that sentence and both passed it. I passed it twice.

#### Finding 2: my round-2 Root Agent `outbox/` claim was wrong, and I withdraw it

Verified: the direct child `ac-root-agent/outbox/` comes from the generic Agent Matrix layout (`entity_creation.rs:128` `AGENT_MATRIX_DIRS`, `:131-134` `create_agent_matrix_subdirs`) and has no production reader or writer. The live queue is `<root>/<agent-local-dir>/outbox/` (`cli/send.rs:827`, `:1091`), and `phone/mailbox.rs:2761-2769` scans exactly that path plus the app outbox, nothing else.

My round-2 evidence row claimed the phrase was "true on both readings". **It was true on neither**: one reading names a directory nothing delivers through, the other silently drops `<agent-local-dir>/`. This is the round-1 failure repeating inside its own correction: a conclusion defended by a reason I had not actually checked. The round-3 text names the nested path explicitly for replicas and omits the Root Agent queue entirely, which the coordinator offered as legitimate and which is the safer choice after getting it wrong once.

#### Finding 3: `send --outbox` escapes both roots

`docs/reference/cli.md:128` documents it publicly; `cli/send.rs:1077-1079` accepts any path and `:1093-1098` creates it and writes the message JSON there. Any categorical "in exactly two locations" claim is false while that option exists.

#### The pattern, and the standard that replaces it

Three rounds, three rewrites, three falsifications, each by an artifact the previous round had not audited. The common factor is not carelessness about any one store; it is that **every version made a categorical claim**, and a universal quantifier converts any single unaudited artifact into a falsification of the whole sentence. The only correct categorical text is an exhaustive one, and exhaustiveness here is a security-documentation audit, not a one-line fix.

**The user decided the scope: bounded and non-categorical wording.** The coordinator relayed it with the operative test, which I applied to every clause of both texts: *would this sentence still be true if a sixth store existed that nobody has audited?* Where the answer was no, the clause was rewritten or removed. The complete audit is issue **#1195**, filed with the validation evidence.

I record my agreement rather than merely my compliance. **The bounded standard is the correct engineering call**, and not only for expediency: a security document that enumerates confidently and incompletely is worse than one that enumerates helpfully and says so, because the first invites a reader to stop looking. Round 2's line would have done exactly that.

#### The grinch's root-cause correction, accepted

It judged my round-2 diagnosis "partly right but incomplete". Correct. I identified the propagation mechanism (a false premise in §4.2's evidence table travelling into §4.6) and stopped there. **The deeper cause is that the inventory treated `phone/messaging.rs`'s filesystem protocol as the whole messaging system** and never audited the API DB queue or the custom-outbox path. That is why the same incomplete premise was still sitting in §4.2, §7 and criteria 20/21/27 after round 2 supposedly fixed it. All four are corrected here.

#### What changed at round 3

Rewritten: §4.6's replacement text and its whole evidence table, now with a "what would falsify it" column; §4.2's replacement text, evidence table and dissent note; §7's persistence and documentation paragraphs; §9.3 rule 9; §9.4's batch-4 `PRIVACY.md` and `docs/security.md` blocks; §8.7 (two files); criteria 20, 21, 27; §10.1 and §10.3's batch-5 rows; §11 decisions 2, 4a, 4b, 4c.
New: §4.6.1 (why two rewrites failed and what the standard is now), §9.3 rule 9 properties (c) and (d), §11 decisions 4d and 4e, §12.2 items 12 and 13, §13.5.
Unchanged: every §5 cut coordinate, every batch-1 through batch-4 instruction, §1.3's line counts, the 13-path scope whitelist, §3, §6, §8.0-§8.6. **No code decision, cut boundary, or batch structure was reopened**; batch 5's file count went from one to two, both already whitelisted. `docs/security.md` stays at 121 lines and `PRIVACY.md` at 54.

#### What I could not close, stated rather than certified around

1. **The corrected text is specified, not implemented.** Batch 5 is the remaining work: two files, four lines.
2. **"Entirely local" and "No external network calls are made" are preserved on instruction.** They are true of AC's own behaviour. A user can weaken them without AC changing, by pointing `--outbox` at a network share or widening `apiServerBind` off loopback. Neither makes the sentence false as written; both belong in #1195. Recorded in §4.2 as dissent, not as a blocker.
3. **Neither document is a complete inventory, and both now say so.** That is the decided outcome, not a gap I am hiding: §7 states it, §4.6 states it in the shipped line, and #1195 owns the audit.

#### Certification (round 3, final)

This plan is certified `READY_FOR_IMPLEMENTATION` as of Step 7 round 3. The certified artifact is the committed file; any byte change after the certifying commit invalidates this status and requires re-certification.
