# Plan #1455: Sequence local TASK writes against session snapshots, keyed on the workgroup that owns the file

Author: architect, wg-14. Draft authored 2026-08-20 UTC as Step 4 of the Full `code-implementation-workflow` path. Normative sections rewritten at Step 7 (round 1) after the Step 6 finding G1.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1455](https://github.com/mblua/AgentsCommander/issues/1455), `TASK header shows stale title after UI title edit (late session snapshot clobbers local save)`.

This is a minimal frontend concurrency fix. It adds two module-scope values and three module-scope functions to `src/terminal/stores/terminal.ts`, one store getter, one store method, one parameter on each of the store's two bind methods, two captured locals in `src/terminal/App.tsx`, two swapped call sites in `src/terminal/components/WorkgroupTask.tsx`, and exactly one new test file. It introduces no new npm dependency, no new module, no new Tauri command, no new event, no new IPC payload shape, no configuration key, and no migration. It touches zero Rust files and adds zero module-to-module import arcs.

Everything asserted below about behaviour was measured, not predicted. See section 13 for the Step 7 measurement record.

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `fix/1455-task-header-stale-clobber`, targeting `main`.

At authoring time (2026-08-20 UTC) the committed `HEAD` of the branch is `1376c2b84a23125624e919c9af7e65813d624241`, equal to the `main` tip given by the dispatch, and `git status --porcelain` is empty. The Codebase Memory index used for the call-graph citations below reports the same `head_sha` (gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-14-dev-v5-team-repo-AgentsCommander`, 24175 nodes, 132915 edges).

Round-1 Step 7 note: all three enrichment passes (Step 5 dev-webpage-ui, Step 6 dev-rust-grinch, Step 7 architect) used the ratified materialise-and-revert pattern against this same SHA and each left `git status --porcelain` empty. Every line number and every measured number in this plan therefore refers to `1376c2b8`.

Root `.gitignore` line 11 ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1455-task-header-stale-clobber.md`. Do not remove or weaken the `plans/` ignore rule.

Step 7 certification re-ran the authority ritual. The implementer must repeat it: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

---

## 2. Issue and objective

Objective: make `terminalStore.activeWorkgroupTask` ordered instead of last-write-wins, and key ownership of a task write on the **workgroup** whose `TASK.md` it rewrote, because that is what the value actually belongs to.

Required outcomes:

- **(A)** A `SessionAPI.list()` snapshot resolving after a local title save can no longer revert the `TASK:` header to the pre-save title.
- **(D)** A local title save resolving after a switch to a session in a **different** workgroup can no longer paint that other workgroup's header.
- **(F, G)** A local title save must still reach the header after a switch to a session in the **same** workgroup, in either resolution order, because both sessions display the same file.
- **(B, C, E, H)** Every behaviour that is correct today stays correct: the snapshot resolving before the save, the no-hydration baseline, a snapshot requested after the save, and a different workgroup's snapshot landing after a save.

Non-goals, binding on the implementer:

- Do not touch `src/sidebar/` or `src-tauri/`. Both were verified correct for this symptom (evidence file 1, sections 4 and 7): the sidebar heals on the 15s discovery snapshot, the emit is a global `app.emit` and reaches both windows, and `SessionInfo.workgroup_task` is re-read from disk on every list (`src-tauri/src/session/session.rs:396`).
- Do not touch the `poll` versus `manual` event-gating asymmetry at `src/terminal/App.tsx:332-350`. The dispatch puts it explicitly out of scope. In particular do not change the `data.sessionIds.includes(targetId)` gate, do not change the `activeWorkingDirectory` gate, and do not route the two event writers through the new store method (section 10, decision D3).
- Do not move, export, or share `normalizePathForCompare` from `App.tsx:326-331`. Section 5.2 records why the store keeps its own copy.
- Do not add a periodic refresh, a poll, or any self-heal timer for the header. The fix is ordering, not redundancy.
- Do not remove or rename `terminalStore.setActiveWorkgroupTask`. It stays, because `src/terminal/App.tsx:337` and `:348` keep using it and `src/shared/testing/ui-harness.tsx:292` calls it during store reset.
- Do not turn the new counter into a Solid signal (section 5.2 records why).
- Do not key ownership on the session id. That was the Step 4 draft, and section 5.9 records the measurement that killed it.
- Do not add tests beyond the one file and the eight cases named in section 9.

---

## 3. Evidence and identified cause

Confirmed at the frozen SHA by direct read of every cited body, by the runtime harness in evidence file 3 with its instrumented log in evidence file 4, and by the Step 5, Step 6 and Step 7 measurement passes.

### 3.1 The header is a pure cache with exactly six writers

`src/terminal/components/WorkgroupTask.tsx:238-244` renders `TASK` + `: ` + `{taskTitle()}`, where `taskTitle` (`:88`) is a memo over `parseTask(terminalStore.activeWorkgroupTask ?? "").title` (`:85-87`). It parses the raw TASK.md content string cached in `src/terminal/stores/terminal.ts:21`. There is no backend-computed title on this path and no periodic refresh.

Complete writer list for that signal at the frozen SHA:

| # | Writer | Value | file:line |
|---|--------|-------|-----------|
| 1 | `saveTitle` (local UI save) | `result.task`, fresh | `WorkgroupTask.tsx:157` |
| 2 | `performClean` (local UI clean) | `result.task`, fresh | `WorkgroupTask.tsx:205` |
| 3 | `bindLive` | `session.workgroupTask` from a `SessionAPI.list()` snapshot | `terminal.ts:182` |
| 4 | `bindLockedSession` (detached window) | same | `terminal.ts:218` |
| 5 | `clearLiveMetadata` | `null` | `terminal.ts:30` |
| 6 | `onWorkgroupTaskUpdated` handler | event payload | `App.tsx:337` (poll), `:348` (manual) |

### 3.2 The value is owned by a workgroup, not by a session

This is the fact the Step 4 draft got wrong, and it determines the whole design. Verified by direct read at the frozen SHA:

- `find_workgroup_task_path_for_cwd` (`src-tauri/src/session/session.rs:242-256`) walks up from a session's `cwd` to the first ancestor directory whose name starts with `wg-`, and returns `<that dir>/TASK.md`.
- `read_workgroup_task_for_cwd` (`:258-264`) reads exactly that path, uncached.
- `SessionInfo::from` calls it on every list: `workgroup_task: read_workgroup_task_for_cwd(&s.working_directory)` (`:396`).
- `resolve_wg_root` (`src-tauri/src/commands/task.rs:38-57`) derives the same root from the same raw `working_directory`, and `TaskUpdateResult.workgroup_root` (`:25-33`) is that root with the `\\?\` prefix stripped by `strip_unc` (`:152-155`).

**Every session under one `wg-*` root therefore reads one shared `TASK.md` and always carries identical `workgroupTask`.** The backend states this itself: `task_set_title`'s doc comment says it emits `workgroup_task_updated` "for sibling sessions/windows" (`task.rs:192-194`).

Two consequences that the design turns on:

1. Two sessions in one workgroup can never legitimately show different task content, so a save made from one of them is the correct content for all of them.
2. `TaskUpdateResult.workgroup_root` is always a literal prefix of the `workingDirectory` of every session in that workgroup, modulo the `\\?\` prefix, slash direction and case, which is exactly what `App.tsx:326-347` already normalises away.

### 3.3 The clobber, confirmed at runtime

`src/terminal/App.tsx:111-120` (`reconcileSelection`) takes the snapshot at the `await` on `:112` and applies it at `:120`:

```
112:      const sessions = await SessionAPI.list();      // snapshot of workgroupTask taken HERE
113:      if (disposed || !terminalStore.matchesSelection(selection, generation)) return true;
...
120:      terminalStore.bindLive(selection, generation, session);   // applied HERE
```

`bindLive` writes the task field unconditionally (`terminal.ts:182`). `loadLockedSession` has the identical shape (`App.tsx:218` await, `:222` bind, `terminal.ts:218`).

The instrumented run (evidence file 4, lines 12-22) shows the failing interleaving verbatim:

```
[step] saveTitle parked on task_set_title
[diag] clearLiveMetadata {"prev":"---\ntitle: Old title\n---\n\nbody\n"}
[diag] clearLiveMetadata {"prev":null}
[step] reconcileSelection parked on list_sessions
[diag] store.setActiveWorkgroupTask {"prev":null,"next":"---\ntitle: New title\n---\n\nbody\n"}
[step] task_set_title resolved with NEW_TASK
[diag] bindLive -> activeWorkgroupTask {"prev":"---\ntitle: New title\n---\n\nbody\n","next":"---\ntitle: Old title\n---\n\nbody\n"}
[step] stale list_sessions snapshot resolved with OLD_TASK
```

The inverted order (case B, log lines 44-54) produces the correct result with identical inputs, and the no-hydration baseline (case C, lines 87-89) is clean. So the outcome is determined purely by which promise settles last: last-write-wins, not a parsing, matching or event-delivery defect. Case B is the falsifier and it falsifies the alternatives.

`reconcileSelection` is not reached only by an explicit session switch. `requestHydration` (`App.tsx:147-191`) calls it on every transport connection-state change through `applyConnectionState` (`:193-214`, `:213`), which is why the user cannot correlate the failure with an action and reports it as intermittent.

### 3.4 Why it stays stale instead of healing

The discovery poll emits `workgroup_task_updated` only on a content change and commits the shipped content into `task_cache` right after a successful emit (`src-tauri/src/commands/ac_discovery.rs:918-960`). The new title was already shipped once, so the poll never re-emits it. The header stays stale until the next real TASK.md edit or the next session switch. The sidebar cannot show the same defect because it is refreshed by both the event and the 15s discovery snapshot.

### 3.5 The second victim of the same unguarded write

`saveTitle` checks `capturedSessionId() !== id` at `WorkgroupTask.tsx:144-147`, before its `await` on `:156`, and never again. The store write at `:157` is therefore unconditional. `performClean` has the same shape (`:196-199` check, `:204` await, `:205` write). Evidence file 4, lines 67-76, shows a save landing after a session switch and painting into the newly bound session's header.

Per section 3.2 this is **two different defects wearing one shape**, and they must not be conflated:

- **Switch to a different workgroup**: the header of workgroup 2 shows workgroup 1's task. Different files, so this is a genuine cross-workgroup display leak. Case D.
- **Switch to a sibling in the same workgroup**: the header shows the content the user just saved, for the very file that session displays. That is **correct**, not a leak. Cases F and G.

The instrumented log's own fixture used two same-workgroup agents with different task content, which section 3.2 shows the backend cannot produce. The defect it demonstrates is real; the fixture attributes it to the wrong axis.

### 3.6 The traps

Two designs look right and are refuted by measurement (section 5.9 has the numbers):

1. **Re-check `activeSessionId` after the await.** `clearLiveMetadata` blanks `activeSessionId` (`terminal.ts:25`) and runs on every selection reserve (`:159`), on every transport generation change or disconnect (`:108`), and from `markUnavailable` (`:190`), `safetySuspendDestroyed` (`:196`) and `suspendLiveBinding` (`:206`). During the in-flight window it is `null`, so the check kills case A's legitimate write.
2. **Key ownership on the session id** (with or without a `selectionId` fallback). It fixes case D and regresses cases F and G, which pass today. That is #1455's own symptom reintroduced on the most common switch in the product, because the sidebar lists agents of a workgroup.

### 3.7 Identified cause, in one sentence

`terminalStore.activeWorkgroupTask` has two asynchronous writers (a local task mutation and a session-list snapshot) that carry no ordering information relative to each other and no record of which `TASK.md` they describe, so whichever promise settles last wins regardless of which value is newer or which file it belongs to.

### 3.8 Call-graph facts that bound the blast radius

From Codebase Memory `trace` at the frozen SHA, corroborated by exhaustive `rg` over `src/` in three independent passes (Step 4, Step 5 S5.9, Step 6):

- `bindLive` has exactly one caller: `reconcileSelection` (`App.tsx:120`). No test calls it.
- `bindLockedSession` has exactly one caller: `loadLockedSession` (`App.tsx:222`). No test calls it.
- `setActiveWorkgroupTask` (the store method) has four callers: `WorkgroupTask.tsx:157`, `WorkgroupTask.tsx:205`, `App.tsx:337`, `App.tsx:348`, plus `src/shared/testing/ui-harness.tsx:292` inside `resetUiStoresForTests`.

Adding a required parameter to either bind method therefore cannot break a call site this plan does not already edit. Confirmed empirically: `npx tsc --noEmit` exits 0 with the change applied.

---

## 4. Scope

### In scope

1. `src/terminal/stores/terminal.ts`: two module-scope race tokens, three module-scope functions, one public getter, one public method `applyLocalTask`, one new parameter on `bindLive`, one new parameter on `bindLockedSession`, two guarded task writes, two extra resets in `resetForTests`.
2. `src/terminal/App.tsx`: capture the token before each of the two awaited `SessionAPI.list()` calls and pass it to the matching bind.
3. `src/terminal/components/WorkgroupTask.tsx`: `saveTitle` and `performClean` write through `applyLocalTask`.
4. One new test file, `src/terminal/App.workgroup-task-race.test.tsx`, with exactly eight cases (A through H).

### Out of scope

- `src/sidebar/`, `src-tauri/`, and every `.rs` file.
- The `poll` versus `manual` gating asymmetry at `App.tsx:332-350`, including the two event writers there.
- `startEditing`'s `TaskAPI.getTitle` await and its own `sessionId() !== id` re-check (`WorkgroupTask.tsx:120-125`). It already re-checks after its await and is not implicated by any confirmed case.
- `TaskAPI.cleanAt` (`src/shared/ipc.ts:673`), which has no caller in `src/terminal/`.
- Any refactor, rename, or "while we are here" improvement.

---

## 5. Decided solution

One design, decided and measured. It is the workgroup-root re-key that Step 6 finding G1 called for, taken one step further than the sketch in G1 (section 5.9 explains the delta and why it is not optional).

### 5.1 Shape

The invariant: **`activeWorkgroupTask` always holds the freshest known content of the `TASK.md` that the currently displayed session belongs to.** Two halves enforce it, both required, neither sufficient alone:

- **Local-write side.** A local task mutation is accepted unless the header is positively known to be showing a **different** workgroup's file. Accepted writes record the workgroup root they rewrote and a monotonic sequence number.
- **Snapshot side.** A bind applies the snapshot's task field only if no accepted local write against **that session's own workgroup** landed after the snapshot was requested. Every other field binds exactly as today.

The counter is what makes the guard expire: because the caller captures it immediately before its `await`, a local write from an earlier, already-observed round can never satisfy `seq > expectedTaskSeq`. Without the counter, one local write would freeze the task field against every future snapshot for that workgroup, which is the same sticky-stale-header symptom from the opposite direction. Case E is the falsifier for that, and it is also the only case that pins the comparison operator (Step 5 measured `>=` breaking E and nothing else).

The asymmetry between the two halves is deliberate and is the part that is easy to get wrong. When the store is **unbound** the workgroup cannot be resolved, because `clearLiveMetadata` has blanked `activeWorkingDirectory` and a `SessionSelection` carries only an id. In that window the local-write side **accepts** rather than rejects, and hands the decision to the bind that blanked the cwd, which does know its own session's workgroup. Rejecting there instead re-breaks case A and, with a `selectionId` fallback, still leaves case G broken (section 5.9).

### 5.2 `src/terminal/stores/terminal.ts`, module scope

Add immediately after the signal declarations (after `terminal.ts:22`):

```ts
// #1455 - `activeWorkgroupTask` is a pure cache with no periodic refresh, so its two
// asynchronous writers (a local TASK mutation and a `SessionAPI.list()` snapshot)
// used to resolve last-write-wins, and a snapshot taken before a save could revert
// the header after it. These two values give the writers an order. Deliberately NOT
// signals: they are race tokens that are never rendered, and a reactive read from a
// memo would be a subscription nobody wants.
let taskWriteSeq = 0;
let lastLocalTaskWrite: { workgroupRoot: string; seq: number } | null = null;

// #1455 - TASK.md is per-WORKGROUP, not per-session: `find_workgroup_task_path_for_cwd`
// (src-tauri/src/session/session.rs:242-256) walks up from a session's cwd to the first
// `wg-*` ancestor, and `SessionInfo.workgroup_task` re-reads that path on every list
// (`session.rs:396`), so every session under one workgroup root shows the same file.
// Ownership of a task write is therefore a workgroup question, never a session one.
// This is the same normalise-and-prefix comparison the manual-event handler already
// uses at src/terminal/App.tsx:326-347; the two must stay in agreement.
function normalizeTaskPath(path: string): string {
  let normalized = path;
  if (normalized.startsWith("\\\\?\\")) normalized = normalized.slice(4);
  else if (normalized.startsWith("//?/")) normalized = normalized.slice(4);
  return normalized.replace(/\\/g, "/").toLowerCase();
}

function cwdUnderWorkgroupRoot(cwd: string, workgroupRoot: string): boolean {
  if (!cwd || !workgroupRoot) return false;
  const normalizedCwd = normalizeTaskPath(cwd);
  const normalizedRoot = normalizeTaskPath(workgroupRoot);
  return (
    normalizedCwd === normalizedRoot ||
    normalizedCwd.startsWith(`${normalizedRoot}/`)
  );
}

// #1455 - true when an accepted local task write landed after the caller took its
// session snapshot AND that session displays the same TASK.md, i.e. the snapshot's
// task field is already stale.
function localTaskWriteWins(sessionCwd: string, expectedTaskSeq: number): boolean {
  return (
    lastLocalTaskWrite !== null &&
    lastLocalTaskWrite.seq > expectedTaskSeq &&
    cwdUnderWorkgroupRoot(sessionCwd, lastLocalTaskWrite.workgroupRoot)
  );
}
```

**On the duplicated normaliser (decided, do not "fix").** `App.tsx:326-331` has a byte-identical `normalizePathForCompare` as a closure inside `onMount`, immediately above the event handler this plan is forbidden to touch. Extracting it to a shared module would edit the out-of-scope block, add a module-to-module arc, and enlarge the diff for a nine-line pure function. The store keeps its own copy under a different name, and the comment above states the agreement requirement. Cost: two copies. Benefit: `App.tsx:326-350` stays byte-identical and section 11 keeps its zero-new-arcs result.

### 5.3 `src/terminal/stores/terminal.ts`, the local-write side

Add to the exported store object, next to `setActiveWorkgroupTask` (`terminal.ts:234-236`), keeping that method unchanged:

```ts
  // #1455 - capture this immediately BEFORE an awaited `SessionAPI.list()` and hand
  // it back to `bindLive` / `bindLockedSession`. Non-reactive on purpose; see the
  // declaration.
  get taskWriteSeq() {
    return taskWriteSeq;
  },

  // #1455 - the local-write side of the task ordering. `workgroupRoot` is the root
  // whose TASK.md the mutation just rewrote, straight off `TaskUpdateResult`. The
  // write is accepted unless the header is positively known to be showing a
  // DIFFERENT workgroup's file. While the store is unbound (`clearLiveMetadata`
  // blanks the cwd on every selection reserve and every transport generation change)
  // the workgroup cannot be resolved, so the write is accepted and the bind that
  // blanked it decides the final value through `localTaskWriteWins`.
  applyLocalTask(workgroupRoot: string, task: string | null): void {
    const cwd = activeWorkingDirectory();
    if (cwd && !cwdUnderWorkgroupRoot(cwd, workgroupRoot)) return;
    taskWriteSeq += 1;
    lastLocalTaskWrite = { workgroupRoot, seq: taskWriteSeq };
    setActiveWorkgroupTask(task);
  },
```

`activeWorkingDirectory` is already a module-scope signal accessor in this file (`terminal.ts:20`). No import is added. Note the method takes no session id: ownership is a workgroup question, so the id would be a term that cannot participate in the decision.

### 5.4 `src/terminal/stores/terminal.ts`, the snapshot side

`bindLive` (`terminal.ts:168-186`) gains a fourth required parameter and one guarded write. Every other line stays as it is:

```ts
  bindLive(
    selection: SessionSelection,
    generation: number,
    session: Session,
    expectedTaskSeq: number,
  ): boolean {
```

and `:182` becomes

```ts
    // #1455 - every other field binds unconditionally; only the task field can lose
    // to a newer local write against the same workgroup's TASK.md.
    if (!localTaskWriteWins(session.workingDirectory, expectedTaskSeq)) {
      setActiveWorkgroupTask(session.workgroupTask ?? null);
    }
```

`bindLockedSession` (`terminal.ts:210-221`) gains a second required parameter:

```ts
  bindLockedSession(session: Session, expectedTaskSeq: number): void {
```

and `:218` becomes the same three-line guard with the comment shortened to `// #1455 - see bindLive.`

The guard is placed after `setActiveWorkingDirectory(session.workingDirectory)` in both methods, but it reads `session.workingDirectory` directly rather than the signal, so its position among the sibling setters is not load-bearing.

### 5.5 `src/terminal/stores/terminal.ts`, reset

`resetForTests` (`terminal.ts:238-249`) gains two lines at the top of its body:

```ts
    taskWriteSeq = 0;
    lastLocalTaskWrite = null;
```

Do not add clearing to `clearLiveMetadata`, `clearLockedSession`, `markUnavailable`, `suspendLiveBinding`, or `safetySuspendDestroyed`. Two independent reasons, the second decisive (D2):

1. The record is self-expiring: `expectedTaskSeq` is captured fresh before every list, so a record from a completed round can never satisfy `seq > expectedTaskSeq`.
2. Lifecycle, verified by exhaustive `rg` at Step 5: `loadLockedSession` has exactly one call site (`App.tsx:270`, inside `onMount`), and `clearLockedSession` has exactly two (`App.tsx:224` and `:229`), both inside `loadLockedSession` itself. The whole locked-session path runs once per window, at mount, strictly before any save is reachable, because the pencil is disabled until `sessionId()` is non-null (`WorkgroupTask.tsx:92-95`). Clearing there is not merely redundant, it is unreachable.

`resetForTests` is reached from `resetUiStoresForTests` (`ui-harness.tsx:291`), so these two lines do close the module-scope leak between test files.

### 5.6 `src/terminal/App.tsx`

In `reconcileSelection`, inside the existing `try` at `:111-112`:

```ts
    try {
      // #1455 - captured BEFORE the await: it is the ordering token the bind
      // compares a later local write against.
      const expectedTaskSeq = terminalStore.taskWriteSeq;
      const sessions = await SessionAPI.list();
```

and `:120` becomes

```ts
      terminalStore.bindLive(selection, generation, session, expectedTaskSeq);
```

In `loadLockedSession`, inside the existing `try` at `:217-218`:

```ts
    try {
      // #1455 - see reconcileSelection.
      const expectedTaskSeq = terminalStore.taskWriteSeq;
      const sessions = await SessionAPI.list();
```

and `:222` becomes

```ts
        terminalStore.bindLockedSession(session, expectedTaskSeq);
```

The capture must sit inside the `try` and immediately above the `await`. Hoisting it above the `try`, or above `reserveSelection`, changes nothing today but breaks the invariant the comment states, so keep it where specified. Step 6 independently confirmed this position is correct for the `onSessionSwitched` path as well as the hydration path, because both funnel through `reconcileSelection`.

### 5.7 `src/terminal/components/WorkgroupTask.tsx`

Exactly two swapped call sites, nothing else in the file changes:

- `:157` `terminalStore.setActiveWorkgroupTask(result.task);` becomes `terminalStore.applyLocalTask(result.workgroupRoot, result.task);`
- `:205` `terminalStore.setActiveWorkgroupTask(result.task);` becomes `terminalStore.applyLocalTask(result.workgroupRoot, result.task);`

`result` is the `TaskUpdateResult` already returned by `TaskAPI.setTitle` and `TaskAPI.clean` (`src/shared/ipc.ts:667-671`), and both commands populate `workgroup_root` (`src-tauri/src/commands/task.rs:241-243` and `:276-278`). No new IPC, no new field, no new command.

Do not remove the pre-await `capturedSessionId()` checks at `:144-147` and `:196-199`: they still produce the user-facing "Session changed; cancel and retry." error, which `applyLocalTask` deliberately does not do (it is a silent store guard, not a UI path).

### 5.8 Stated dependency on backend behaviour

The plan edits no Rust, but its correctness rests on three facts that live in `src-tauri/` and are recorded here so a later change cannot break them silently (Step 5 S5.7, extended by section 3.2):

1. **`TASK.md` is per-workgroup and resolved by the `wg-*` walk-up** (`session.rs:242-264`). If a session ever gained a private task file, the workgroup key would become too coarse and cases F and G would become wrong.
2. **`task_set_title` and `task_clean` commit to disk before their promise resolves** (`task.rs:218-243`, `:259-281`: `task_ops::perform` writes, then `emit_task_updated`, then `Ok(result)`). If either became write-behind or debounced, "requested after the local write" would stop implying "at least as fresh" and the guard would silently stop protecting case A while every test here stayed green, because the harness mocks the transport.
3. **`SessionInfo.workgroup_task` is re-read from disk on every list, uncached** (`session.rs:396`). A cache there would make snapshots stale in a way no ordering token can detect.

### 5.9 Rejected alternatives, with the measurements that killed them

Each row was applied to the working tree at the frozen SHA and run against the section 9.1 suite. Full record in section 13.

| Alternative | Result | Verdict |
|---|---|---|
| Re-check `activeSessionId() === id` inside `saveTitle` after the await, no store change | Case A fails (measured at Step 5 as a predicate mutation) | Rejected. In A's window `activeSessionId` is `null`, so the check suppresses the legitimate write. |
| Key the owner on the session id, with `(activeSessionId() ?? selectionId())` as the write test and `lastLocalTaskWrite.sessionId === sessionId` as the bind test (the Step 4 draft) | **Cases F and G fail** | Rejected. F passes at the frozen SHA, so this is a regression, and it is #1455's own symptom on the most common switch in the product. |
| The Step 6 G1 sketch: workgroup root OR'd with the session-id fallback (`if (!pointsAtWorkgroup && (activeSessionId() ?? selectionId()) !== id) return;`) | Fixes F, **leaves G broken** | Rejected. With the cwd blanked, `cwdUnderWorkgroupRoot` is false and the surviving id term rejects a sibling's legitimate save, so the pre-save snapshot then binds and the header goes stale. Section 5.1's asymmetry is what closes it. |
| Owner-only guard on the bind side (skip the task whenever any local write exists for that workgroup) | Case E fails | Rejected. It never expires, so the header would ignore every later snapshot for that workgroup. |
| Comparison operator `>=` instead of `>` | Case E fails (measured at Step 5) | Rejected. Off-by-one; E is the operator gate. |
| Compare content instead of order (skip the bind when `session.workgroupTask` differs from what we hold) | not run | Rejected by construction: it cannot tell "the snapshot is stale" from "someone else edited TASK.md", so it would suppress every legitimate external update. |
| A periodic refresh, or a re-emit from the discovery poll | not run | Rejected: changes `src-tauri/`, out of scope, and papers over the ordering defect with latency. |
| Make `taskWriteSeq` a Solid signal | not run | Rejected: it is read only from non-reactive async code, and a signal invites an accidental subscription that would re-run memos on every task write. Step 6 independently confirmed no subscription is created today. |

---

## 6. Affected surfaces, exhaustively

Modified:

| File | Symbols | Nature |
|---|---|---|
| `src/terminal/stores/terminal.ts` | module scope after `:22`; `bindLive` `:168`, `:182`; `bindLockedSession` `:210`, `:218`; store object near `:234`; `resetForTests` `:238` | +2 module values, +3 module functions, +1 getter, +1 method, +2 params, 2 guarded writes, +2 reset lines |
| `src/terminal/App.tsx` | `reconcileSelection` `:111-120`; `loadLockedSession` `:217-222` | +2 captured locals, 2 edited call sites |
| `src/terminal/components/WorkgroupTask.tsx` | `saveTitle` `:157`; `performClean` `:205` | 2 edited call sites |

Created:

- `src/terminal/App.workgroup-task-race.test.tsx`

Measured diffstat over `src/` with the change applied: `App.tsx` +9/-3 in 2 hunks, `WorkgroupTask.tsx` +2/-2, `terminal.ts` +88/-3. Total 93 insertions, 8 deletions across the three files.

Explicitly not touched: `src/sidebar/`, `src-tauri/`, `src/shared/types.ts`, `src/shared/ipc.ts`, `src/shared/testing/*`, `src/terminal/App.tsx:326-352` (the normalise helper and the event handler), `src/terminal/components/TerminalView.tsx`, and every existing test file.

Public surface delta: `terminalStore` gains `taskWriteSeq` (getter) and `applyLocalTask(workgroupRoot, task)`. `bindLive` and `bindLockedSession` gain one required parameter each. `setActiveWorkgroupTask` is unchanged and still exported.

---

## 7. Required behavior, edge cases, failure behavior

Each numbered behaviour names the case that pins it, or says plainly that nothing does.

1. **Local save while bound to a session of that workgroup** (cases B, C): accepted, header shows the new title. Unchanged from today.
2. **Local save while the store is unbound** (case A, and the first half of G and H): accepted, because the workgroup cannot be resolved and the in-flight bind will decide. This is the legitimate write that both refuted designs killed.
3. **Local save while bound to a session of a DIFFERENT workgroup** (case D): silently dropped from the store. The backend write already happened and is correct on disk; only the display of a workgroup the user has left is suppressed. `saveTitle` still clears its editing state and shows no error, exactly as today, because `applyLocalTask` returns without throwing.
4. **Local save while bound to a SIBLING of the same workgroup** (case F): accepted. The sibling displays the very file that was edited, so this is the correct content for it.
5. **Snapshot whose list was requested before an accepted local write for the same workgroup** (cases A, G): the task field is skipped; `activeSessionId`, `activeSessionName`, `activeShell`, `activeShellArgs`, `activeWorkingDirectory`, `activeIsRootAgent` and `bindingState` all bind normally, and `bindLive` still returns `true`. Case A asserts the four survivors explicitly.
6. **Snapshot whose list was requested after that write** (case E): applied normally. The guard expires.
7. **Snapshot for a different workgroup than the last local write** (case H): applied normally, because the workgroup does not match. Under the Step 4 draft the equivalent term was unreachable dead code (Step 6 finding G4); under this design it is reachable and pinned.
8. **Detached window first load**: `loadLockedSession` runs once from `onMount` (`App.tsx:270`) and is the only caller of `bindLockedSession`. At that moment `taskWriteSeq` is 0 and `lastLocalTaskWrite` is `null`, so the guard is provably inert and behaviour is byte-for-byte today's. The parameter is added for symmetry and because the issue names the path. **No case targets it, deliberately**, because the state is unreachable (section 9.1 coverage note).
9. **Event-driven writes** (`App.tsx:337`, `:348`): unchanged, still `setActiveWorkgroupTask`, still unsequenced. Section 10, decision D3 and residual R1.
10. **`clearLiveMetadata` blanking the header to `null`**: unchanged. The user-visible sequence for case A becomes "title blanks, then the new title appears", where today it is "title blanks, then the OLD title appears". The blank flicker itself is pre-existing and out of scope (R3).
11. **Failure behavior**: `applyLocalTask` never throws and never logs. A dropped write is indistinguishable from today's behaviour except in case D, where today's behaviour is the defect. `bindLive` and `bindLockedSession` keep their existing return contracts and their existing early-return guards.
12. **Degenerate inputs**: `cwdUnderWorkgroupRoot` returns `false` for an empty cwd or an empty root, so a malformed `workgroupRoot` makes a bound write reject rather than match everything. `result.task` may legitimately be `null` (`TaskUpdateResult.task` is `Option<String>`), and `applyLocalTask` accepts `string | null`.
13. **Overflow**: `taskWriteSeq` is a JS number incremented once per accepted local task write, so `Number.MAX_SAFE_INTEGER` is unreachable in any real session. No wrap handling.
14. **Nested workgroups**: `cwdUnderWorkgroupRoot` is a prefix test, while the backend uses the FIRST `wg-*` ancestor. They differ only if one `wg-*` directory is nested inside another, which the `.ac/wg-N-name/__agent_x` layout does not produce. The same imprecision already ships in the `manual` event handler at `App.tsx:342-347`, so this introduces no new class of error. Not tested.

---

## 8. Compatibility and security

- No IPC surface change: no Tauri command, event name, or payload shape is added, removed, or altered. `src/shared/types.ts` and `src/shared/ipc.ts` are untouched, so the Rust and TypeScript sides stay in sync by construction. `TaskUpdateResult.workgroupRoot` is an existing field that the frontend simply starts reading.
- No persisted state, no config key, no migration. Both new values are in-memory, per-webview, and reset on reload.
- No new npm dependency and no version change.
- No change to the PTY input or output flow.
- **Security, scoped precisely.** The only display-confidentiality issue here is **cross-workgroup**: today a save made in workgroup 1 can paint into the header of a session in workgroup 2, which shows one workgroup's task text under another's. Case D closes that. Within a single workgroup there is nothing to close, because every session shows the same file; the change deliberately keeps writing there (cases F and G), and describing that as "narrowing what can be displayed" would be wrong. Nothing new is written to disk, sent over IPC, or logged, and no path is widened.
- Windows specifics: the only new path handling is a normalise-and-prefix comparison that strips the `\\?\` extended-length prefix and lower-cases, matching both `strip_unc` on the Rust side (`task.rs:152-155`) and the existing frontend comparison at `App.tsx:326-331`. No shell wrapping and no ConPTY interaction is touched.

---

## 9. Tests and objective acceptance criteria

### 9.1 The one new test file

Create `src/terminal/App.workgroup-task-race.test.tsx`. The content below is byte-exact: it is the file that produced every number in section 9.2 and section 13, so implementation step 1 is a copy with no editing.

Every helper it needs already exists and is already imported by `src/terminal/App.workflow.test.tsx`, so no new module arc is created: `baseSettings`, `click`, `input`, `installBrowserDomStubs`, `renderWithFakeTransport`, `resetUiStoresForTests`, `session`, `waitFor` from `../shared/testing/ui-harness`, `FakeTransport` from `../shared/testing/fake-transport`, and `liveSelection`, `SESSION_A`, `SESSION_B` from `../shared/testing/session-selection`.

**Why eight cases (D5).** Four are single-session; the other four are the 2x2 switch matrix that section 3.2 forces. Each cell kills a distinct wrong implementation, and none is redundant:

| Case | Interleaving | The wrong implementation it kills |
|---|---|---|
| A | one session, snapshot requested before the save, binds after it | "the snapshot always wins" (today's defect) |
| B | one session, snapshot binds before the save resolves | "the local write is dropped whenever a bind happened" |
| C | one session, no hydration | "the guard rejects the ordinary save" |
| E | one session, snapshot requested after the save | "the guard never expires", and the `>=` off-by-one (D6) |
| D | different workgroup, snapshot binds first | "accept every local write" (today's cross-workgroup leak) |
| F | same workgroup, snapshot binds first | "key ownership on the session id" on the WRITE side |
| G | same workgroup, save resolves first | "key ownership on the session id" on the SNAPSHOT side, and any session-identity fallback in the unbound window |
| H | different workgroup, save resolves first | "suppress every snapshot after a local write" |

**Coverage notes, stated plainly rather than implied.**

- No case targets `bindLockedSession`. Its only caller runs once from `onMount` before any session is bound, so the pencil that starts a save is still disabled and the race is unreachable there (section 7 item 8). Adding a case would test a state the app cannot enter.
- Every selection change in the suite is produced by `forceHydration`, which bumps the transport connection generation and reaches `reconcileSelection` through `applyConnectionState` and `requestHydration`. A real sidebar click reaches `reconcileSelection(selection, deliveryGeneration, false)` directly from the `onSessionSwitched` listener (`App.tsx:240-241`) with no generation change. Both funnel through `reserveSelection` and `clearLiveMetadata`, and Step 6 confirmed the guard cannot behave differently between them, but the suite drives **a reconnect that returns a different selection**, not a sidebar click. Criterion 10b is the manual check that covers the real click.
- Case H asserts only its final state, not the sub-frame transient described in R2, so a future improvement that resolves the pending session's workgroup earlier is not blocked by the suite.

```tsx
// @vitest-environment jsdom
//
// #1455 regression suite. `terminalStore.activeWorkgroupTask` is a pure cache with
// no periodic refresh, so its two asynchronous writers (a local TASK mutation and a
// `SessionAPI.list()` snapshot) must be sequenced instead of resolving
// last-write-wins.
//
// TASK.md is per-WORKGROUP, not per-session (`find_workgroup_task_path_for_cwd`,
// src-tauri/src/session/session.rs:242-256), so every session under one `wg-*` root
// shows the same file. Cases D, F, G and H are the 2x2 switch matrix that pins that:
// {same workgroup, different workgroup} x {snapshot lands first, save resolves first}.
//
// It drives the REAL TerminalApp -> reconcileSelection -> bindLive and the REAL
// WorkgroupTask -> saveTitle. The only thing mocked is the transport boundary.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "./App";
import { terminalStore } from "./stores/terminal";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { liveSelection, SESSION_A, SESSION_B } from "../shared/testing/session-selection";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    destroy: vi.fn(() => Promise.resolve()),
    onCloseRequested: vi.fn(() => Promise.resolve(() => undefined)),
  }),
}));

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  resize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({ instances: [] as FakeTerminalInstance[] }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    constructor() {
      xterm.instances.push(this);
    }
    loadAddon(addon?: { activate?: (terminal: FakeTerminalInstance) => void }): void {
      addon?.activate?.(this);
    }
    open(element: HTMLElement): void {
      this.element = element;
    }
    focus(): void {}
    dispose(): void {}
    write(_data: unknown, callback?: () => void): void {
      callback?.();
    }
    reset(): void {}
    scrollToBottom(): void {}
    paste(): void {}
    hasSelection(): boolean {
      return false;
    }
    getSelection(): string {
      return "";
    }
    clear(): void {}
    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
    }
    onData(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    onResize(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    onSelectionChange(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    attachCustomKeyEventHandler(): void {}
    registerLinkProvider(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    get buffer() {
      return { active: { cursorY: 0, viewportY: 0, length: 0, getLine: () => null } };
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;
    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }
    fit = vi.fn(() => {
      this.terminal?.resize(88, 26);
    });
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    onContextLoss = vi.fn();
    dispose = vi.fn();
  },
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false }));

const WG_ROOT = "C:\\Project\\.ac\\wg-1-dev-team";
const WG_CWD = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect";
const SIBLING_CWD = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-rust";
const OTHER_WG_CWD = "C:\\Project\\.ac\\wg-2-other-team\\__agent_dev-rust";
const OLD_TASK = "---\ntitle: Old title\n---\n\nbody\n";
const NEW_TASK = "---\ntitle: New title\n---\n\nbody\n";
const OTHER_WG_TASK = "---\ntitle: Other workgroup task\n---\n\nbody\n";
const EXTERNAL_TASK = "---\ntitle: External title\n---\n\nbody\n";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function wgSession(workgroupTask: string | null) {
  return session({
    id: SESSION_A,
    name: "wg-1-dev-team/architect",
    workingDirectory: WG_CWD,
    workgroupTask,
  });
}

/** Sibling agent of the SAME workgroup. It reads the same TASK.md as SESSION_A, so
 *  the backend can only ever give it identical `workgroupTask` content. */
function siblingSession(workgroupTask: string | null) {
  return session({
    id: SESSION_B,
    name: "wg-1-dev-team/dev-rust",
    workingDirectory: SIBLING_CWD,
    workgroupTask,
  });
}

/** Agent of a DIFFERENT workgroup, so a different TASK.md and different content. */
function otherWorkgroupSession(workgroupTask: string | null) {
  return session({
    id: SESSION_B,
    name: "wg-2-other-team/dev-rust",
    workingDirectory: OTHER_WG_CWD,
    workgroupTask,
  });
}

function setupTransport(fake: FakeTransport, listSessions: () => unknown): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.onInvoke("list_sessions", listSessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.onInvoke("activate_terminal_output", (args) => ({
    sessionId: String(args.sessionId),
    data: [],
    rows: 24,
    cols: 80,
    sequence: 0,
  }));
  fake.resolve("detach_terminal_output", undefined);
}

function headerTitle(root: HTMLElement): string | null {
  return root.querySelector(".workgroup-task-title")?.textContent ?? null;
}

async function flush(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) await Promise.resolve();
}

/** Click the pencil, type the new title, click Save. Returns once saveTitle is
 *  parked on its `await TaskAPI.setTitle(...)`. */
async function startSave(root: HTMLElement, title: string): Promise<void> {
  const editButton = root.querySelector<HTMLButtonElement>(
    'button.workgroup-task-action[title="Edit TASK title"]',
  );
  expect(editButton, "edit (pencil) button").toBeTruthy();
  expect(editButton!.disabled, "pencil must be enabled while bound").toBe(false);
  click(editButton!);

  await waitFor(() =>
    expect(root.querySelector(".workgroup-task-title-input")).toBeTruthy(),
  );
  const titleInput = root.querySelector<HTMLInputElement>(
    ".workgroup-task-title-input",
  )!;
  input(titleInput, title);

  const saveButton = root.querySelector<HTMLButtonElement>(
    "button.workgroup-task-title-btn.save",
  )!;
  expect(saveButton.disabled).toBe(false);
  click(saveButton);
  await flush();
}

/** Force a fresh connection generation while still connected. This is what
 *  `applyConnectionState` turns into requestHydration -> reconcileSelection. */
async function forceHydration(fake: FakeTransport, generation: number): Promise<void> {
  fake.setConnectionState({ state: "connected", generation });
  await flush();
}

describe("#1455 TASK header write ordering", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
    vi.useRealTimers();
  });

  it("CASE A: a stale list() snapshot resolving AFTER the save must not revert the header", async () => {
    const fake = new FakeTransport();
    const staleList = deferred<unknown>();
    let holdList = false;
    let heldCalls = 0;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      heldCalls += 1;
      return staleList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      holdList = true;
      await forceHydration(fake, 1);
      expect(heldCalls, "hydration must have issued a held list_sessions").toBe(1);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);

      staleList.resolve([wgSession(OLD_TASK)]);
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
      expect(terminalStore.activeWorkingDirectory).toBe(WG_CWD);
      expect(terminalStore.bindingState).toBe("bound");
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE B: the same snapshot resolving BEFORE the save keeps the new title", async () => {
    const fake = new FakeTransport();
    const staleList = deferred<unknown>();
    let holdList = false;
    let heldCalls = 0;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      heldCalls += 1;
      return staleList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      holdList = true;
      await forceHydration(fake, 1);
      expect(heldCalls).toBe(1);

      staleList.resolve([wgSession(OLD_TASK)]);
      await flush(10);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE C: no hydration at all, the save sticks (baseline)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => ({
      workgroupRoot: WG_ROOT,
      task: NEW_TASK,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));
      await startSave(rendered.root, "New title");
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE D: switch to a DIFFERENT workgroup, snapshot first, the save must be dropped", async () => {
    const fake = new FakeTransport();
    const otherList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return otherList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);
      otherList.resolve([otherWorkgroupSession(OTHER_WG_TASK)]);
      await flush(10);
      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(headerTitle(rendered.root)).toBe("Other workgroup task");

      // wg-1's save now returns while a wg-2 session is bound. Different TASK.md,
      // so painting it here would display one workgroup's task under another's.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(OTHER_WG_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Other workgroup task"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE E: a snapshot taken AFTER the save still refreshes the header (the guard expires)", async () => {
    const fake = new FakeTransport();
    let listTask = OLD_TASK;
    setupTransport(fake, () => [wgSession(listTask)]);
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => ({
      workgroupRoot: WG_ROOT,
      task: NEW_TASK,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);

      listTask = EXTERNAL_TASK;
      await forceHydration(fake, 1);
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(EXTERNAL_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("External title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE F: switch to a SAME-workgroup sibling, snapshot first, the save must still paint", async () => {
    const fake = new FakeTransport();
    const siblingList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return siblingList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);
      // The sibling's snapshot was served before the write committed, so it carries
      // the pre-save content of the SHARED file.
      siblingList.resolve([siblingSession(OLD_TASK)]);
      await flush(10);
      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(headerTitle(rendered.root)).toBe("Old title");

      // The save returns. The sibling displays the very file that was edited, so
      // dropping this write would leave #1455's own symptom in place.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE G: switch to a SAME-workgroup sibling, save first, the stale sibling snapshot must lose", async () => {
    const fake = new FakeTransport();
    const siblingList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return siblingList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);

      // The save resolves while the store is unbound and the sibling's list is
      // still in flight.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      // Then the pre-save sibling snapshot lands. Same file, older content.
      siblingList.resolve([siblingSession(OLD_TASK)]);
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE H: switch to a DIFFERENT workgroup, save first, the new workgroup's snapshot must win", async () => {
    const fake = new FakeTransport();
    const otherList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return otherList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      // wg-2's snapshot is a different TASK.md, so the wg-1 write must not suppress it.
      otherList.resolve([otherWorkgroupSession(OTHER_WG_TASK)]);
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(OTHER_WG_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Other workgroup task"));
    } finally {
      rendered.cleanup();
    }
  });
});
```

### 9.2 Objective acceptance criteria

All commands run from the repo root on branch `fix/1455-task-header-stale-clobber`. Every number below was measured at Step 7 against the frozen SHA, so these are reproduced readings, not predictions.

1. **The suite is red before the source change, in a specific shape.** Land the test file first (implementation order step 1) and run
   `npx vitest run src/terminal/App.workgroup-task-race.test.tsx`
   Expected: `Tests 3 failed | 5 passed (8)`, and the three failures must be exactly **A, D and G**. If any other case fails, or if fewer than three fail, stop: the test file is not exercising the defect and the implementation is not authorized to proceed.
   Cases B, C, E, F and H passing before the source change is correct and required: they pin behaviour that must survive. **Case F passing here is the load-bearing one**, because it is the behaviour a session-id-keyed design regresses.
2. **The suite is green after the source change.** `npx vitest run src/terminal/App.workgroup-task-race.test.tsx` gives `Test Files 1 passed (1)`, `Tests 8 passed (8)`.
3. **The suite rejects the wrong owner key** (the criterion Step 6 finding G2 asked for). This is a one-off verification, not a permanent gate. With the change applied, temporarily re-key ownership on the session id: make `lastLocalTaskWrite` hold `{ sessionId, seq }`, make `localTaskWriteWins` compare `lastLocalTaskWrite.sessionId === sessionId`, make `applyLocalTask(id, task)` guard on `(activeSessionId() ?? selectionId()) !== id`, and pass `session.id` / `id` at the three call sites. Expected: `Tests 2 failed | 6 passed (8)` with **F and G** as the failures. Revert the probe. If the probe passes all eight, the suite has lost its ability to detect the regression and must not be certified.
4. **No existing test regresses.** `npm test` gives `Test Files 159 passed (159)`, `Tests 1584 passed (1584)`. The frozen-SHA baseline is `158 (158)` and `1576 (1576)`, so the delta is exactly `+1` file and `+8` tests.
5. **Types.** `npm run typecheck` exits 0. This is the gate that proves all three bind/write call sites were updated, since the new parameters are required.
6. **Frontend dependency-cycle gate.** `npm run check:frontend-dependencies` prints `fixture matrix verdicts: PASS`, `modules: 343, errors: 0, dependencies: 1493`, `complete-root gate: PASS` and `check:frontend-dependencies OK`. The frozen-SHA baseline is `modules: 342, errors: 0, dependencies: 1487`, so the delta is exactly `+1` module and `+6` dependencies (the new file's five relative imports plus `vitest`). The module count is filesystem-derived, so it reads `343` before `git add -f` as well as after. Any nonzero `errors` fails the gate. This script is not wired to CI, so it must be run by hand.
7. **Rust arc record untouched.** `git diff --stat main...HEAD -- src-tauri/` produces empty output, so `src-tauri/module-arcs.txt` is byte-identical to the frozen SHA.
8. **Scope.** `git diff --name-only main...HEAD` lists exactly: `plans/1455-task-header-stale-clobber.md`, `src/terminal/App.tsx`, `src/terminal/App.workgroup-task-race.test.tsx`, `src/terminal/components/WorkgroupTask.tsx`, `src/terminal/stores/terminal.ts`. Nothing else.
9. **Test debt.** `npm run test:debt` exits 0. It also prints a non-empty allowlist of pre-existing `placeholder-rust-test` entries under `src-tauri/src/phone/mailbox.rs`; that output is expected at the frozen SHA and is not a failure.
10. **Manual confirmation (recommended, not blocking).** Two checks in the running app, both five seconds:
    a. Edit a coordinator's TASK title while forcing a reconnect. Per evidence file 2 section 3 the correct post-fix sequence is: the title blanks, then the NEW title appears. A blank followed by the OLD title means the fix did not take.
    b. **The sibling-switch check** (Step 6 finding G2). Save a new title, then immediately click a different agent **in the same workgroup**. The header must show the new title, not the pre-save one. This is the common path and it is the one a session-id-keyed implementation breaks.

---

## 10. Explicit decisions and accepted residuals

**D1. Ownership is keyed on the workgroup root, not the session id.** Decided at Step 7 on finding G1. `TASK.md` is per-workgroup (section 3.2), so the session id cannot participate in the decision. `applyLocalTask` therefore takes no id at all. Measured: the session-id key fails cases F and G (section 5.9, section 13).

**D2. The local-write side accepts while the store is unbound, instead of falling back to a session-id test.** Decided at Step 7. This is the delta over the Step 6 G1 sketch. With the cwd blanked the workgroup is unknowable, and any session-identity fallback rejects a sibling's legitimate save, leaving case G broken. Measured: the sketch fixes F and leaves G failing. Cost: residual R2.

**D3. `lastLocalTaskWrite` is cleared only in `resetForTests`.** Two reasons, the lifecycle one decisive (section 5.5). Recorded so a future reader does not "restore" clearing on lifecycle grounds and reintroduce a reset the sequence comparison was designed to make unnecessary.

**D4. The two event writers at `App.tsx:337` and `:348` are NOT routed through `applyLocalTask`.** They sit inside the block the dispatch puts out of scope; and for a detached window before its first bind, routing them would change behaviour with no test behind it.

**D5. The suite has eight cases.** Four single-session cases (A, B, C, E) plus the full 2x2 switch matrix: {same workgroup, different workgroup} x {snapshot binds first, save resolves first} = D, F, G, H. The matrix is not padding; section 9.1's coverage note names the distinct wrong implementation each cell kills, and two of the four cells kill designs that were actually proposed during this plan's review.

**D6. Case E is the operator gate, not just the expiry falsifier.** Step 5 measured `>=` instead of `>` breaking E and nothing else. An off-by-one is the most likely single mistake in section 5.2, and E is the entire net that catches it.

**D7. Step 5's case F probe is superseded, not rejected.** Dev-webpage-ui built a falsifier for the `(activeSessionId() ?? selectionId())` tightening. That predicate no longer exists (D1, D2), so the probe has nothing left to pin. Its purpose, verifying behaviour inside the reserve window where the store is unbound, is now served by cases G and H, which drive exactly that window in both workgroup directions and assert the final state. The letter F is reused for the same-workgroup snapshot-first case.

**R1 (accepted residual).** A `workgroup_task_updated` event write can still be clobbered by a concurrently in-flight snapshot, because those writers are unsequenced (D4). Narrower than the reported defect (the event is dropped entirely during the in-flight window anyway, per evidence file 1 section 5), never reported, and belongs with the out-of-scope gating rework at `App.tsx:332-350`.

**R2 (accepted residual, the cost of D2).** During a switch to a **different** workgroup, a save resolving while the store is unbound is accepted, so for a sub-frame the header can show the previous workgroup's new title while nothing is bound and the terminal shows "Loading session...". The in-flight bind then applies the new workgroup's task. Case H drives exactly this interleaving and asserts the final state; the transient is deliberately not asserted, so a future improvement that resolves the pending session's workgroup earlier would not be blocked by the suite. Closing it properly would require the selection coordinator to publish the pending session's cwd, which is a much larger change.

**R3 (accepted residual).** The blank flicker before the header repaints is pre-existing (`clearLiveMetadata` sets the task to `null`) and out of scope. The fix changes what follows the blank, not the blank.

**R4 (accepted residual, from Step 5 S5.8 and Step 6 G6).** `markUnavailable` (`terminal.ts:190`), `suspendLiveBinding` (`:206`) and `safetySuspendDestroyed` (`:196`) all call `clearLiveMetadata()`, which blanks the cwd. Under D2 a local save resolving after any of them is therefore accepted, and paints a title while the store has just declared the session unavailable. `safetySuspendDestroyed` is the sharpest form: the session is destroyed, yet `<WorkgroupTask />` keeps rendering because `App.tsx:402` gates only on `activeIsRootAgent`. This is **not** a regression, since today's unconditional write does the same, and it self-heals on the next `markUnavailable`, which clears the task to `null`. Closing it would mean deciding what the header should show for an unavailable session, a UX question this issue did not ask.

**R5 (accepted residual, from Step 6 G7).** The sequence token compares **request** time, not read time. "Requested after the local write" implies "at least as fresh" (section 5.8 fact 2), but the converse does not hold: a list issued at T0 can read `TASK.md` at T3, so an external edit committed between the local write and T3 is genuinely fresher yet is still suppressed. Consequence: one external `TASK.md` edit can be dropped from the header, healing on the next hydration or session switch. Strictly narrower than the defect being fixed. Recorded so a later reader does not mistake the seq guard for a freshness guarantee.

**Resolved by the re-key, not carried as residuals.** Step 6 finding G4 reported that the `sessionId` term of `localTaskWriteWins` was unreachable dead code under the Step 4 draft. Under D1 the equivalent term is `cwdUnderWorkgroupRoot(session.workingDirectory, ...)`, which is both reachable and load-bearing in both directions: case G pins it true across a sibling, case H pins it false across a workgroup boundary. Section 7 item 7 cites them.

---

## 11. Dependency-cycle and layering statement (planning rule 8)

Applying `verify-no-dependency-cycles` at the frozen SHA, re-verified at Step 7 with the change applied:

- **New module-to-module arcs: zero.** No `import` statement is added to any existing file. `src/terminal/stores/terminal.ts` gains only module-local code and reuses the `activeWorkingDirectory` accessor already declared in that same file (`:20`). `src/terminal/App.tsx` already imports `terminalStore` (`:22`). `src/terminal/components/WorkgroupTask.tsx` already imports `terminalStore` (`:3`). This is the reason section 5.2 duplicates the nine-line path normaliser instead of extracting it to a shared module: extraction would create an arc and edit the out-of-scope block.
- **The one new file adds one node, not one arc class.** `src/terminal/App.workgroup-task-race.test.tsx` imports `./App`, `./stores/terminal`, `../shared/testing/fake-transport`, `../shared/testing/ui-harness` and `../shared/testing/session-selection`. Every one of those five edges already exists from `src/terminal/App.workflow.test.tsx`, so no previously-clean module boundary is crossed and no SCC gains a member.
- **Removed arcs: zero.**
- **SCC impact: none**, and this is measured rather than argued. `npm run check:frontend-dependencies` over the complete `src` root under the `no-circular` rule reports `errors: 0` at the frozen SHA (`modules: 342, dependencies: 1487`) and `errors: 0` with the change applied (`modules: 343, dependencies: 1493`). The delta is exactly the new test file and its six edges.
- **Rust side: not applicable and provably untouched.** No `.rs` file is edited, so `src-tauri/module-arcs.txt` stays byte-identical (criterion 7 of section 9.2). The `cyclicSccs`-unchanged criterion is satisfied vacuously.
- **Role and layering hygiene.** No lower-layer module gains a UI-transport dependency: nothing here imports `@tauri-apps/api`, and `src/terminal/stores/terminal.ts` keeps its existing three imports (`solid-js`, `../../shared/types`, `../../shared/transport`), all type-only except `createSignal`. The three new module-scope functions are pure: two string predicates and one comparison over module state, none of them transport-aware. The transport-taking code stays where it already is, in `App.tsx` and `WorkgroupTask.tsx`. The `no-terminal-helper-back-edge` rule in `dependency-cruiser.config.mjs` scopes its `from` to `terminal-session-registry.ts` and `terminal-output-admission.ts`, neither of which this plan touches.

**Gate result: PASS.** The plan adds no module dependency cycle, grows no SCC, joins no SCCs, and adds no arc crossing a previously-clean SCC boundary.

---

## 12. Implementation order

1. Create `src/terminal/App.workgroup-task-race.test.tsx` exactly as in 9.1. Run `npx vitest run src/terminal/App.workgroup-task-race.test.tsx`. It must be `3 failed | 5 passed (8)` with A, D and G the failures. If it is not, stop and report before touching any source file.
2. Apply the `src/terminal/stores/terminal.ts` changes (5.2, 5.3, 5.4, 5.5) in one edit. The file will not typecheck yet, because the two bind call sites still pass too few arguments; that is expected and is not measured as a gate.
3. Apply the `src/terminal/App.tsx` changes (5.6).
4. Apply the `src/terminal/components/WorkgroupTask.tsx` changes (5.7).
5. `npm run typecheck` (must exit 0), then `npx vitest run src/terminal/App.workgroup-task-race.test.tsx` (must be 8 passed).
6. Run the one-off owner-key probe of criterion 3, confirm `2 failed | 6 passed` with F and G failing, and revert the probe.
7. `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies`. Compare against the exact numbers in 9.2.
8. Commit, including `git add -f plans/1455-task-header-stale-clobber.md`.

---

---

## Step 5 enrichment (dev-webpage-ui)

Author: dev-webpage-ui, wg-14. Appended 2026-08-20 UTC as Step 5 of the Full path. Architect's
sections above are unmodified. Nothing here changes a decided ruling; where I disagree or find a
gap I say so and leave the call to the tech lead.

Method note: everything below is measured, not reasoned. I extracted section 9.1's test file
verbatim into `src/terminal/App.workgroup-task-race.test.tsx`, applied sections 5.2 through 5.7
verbatim, ran the gates, mutated the guard to probe coverage, then reverted all of it. The tree is
back to a clean `fix/1455-task-header-stale-clobber` at the frozen SHA, and no source file in this
plan's scope was left modified.

### S5.1 The plan is implementable exactly as written (verified end to end)

| Gate | Frozen SHA (before) | Plan applied (after) |
|---|---|---|
| `npx tsc --noEmit` | 0 | 0 |
| `npx vitest run src/terminal/App.workgroup-task-race.test.tsx` | 3 passed, 2 failed (A and D fail) | 5 passed (5) |
| `npm test` | `Test Files 158 passed (158)`, `Tests 1576 passed (1576)` | `Test Files 159 passed (159)`, `Tests 1581 passed (1581)` |
| `npm run check:frontend-dependencies` | `modules: 342, errors: 0, dependencies: 1487` | `modules: 343, errors: 0, dependencies: 1493` |
| `npm run test:debt` | exit 0 | not re-measured (no test-debt-visible change) |

Section 9.2 criterion 1 is therefore not a prediction, it is a reproduced result: the extracted
file compiles and yields exactly 3 passed / 2 failed with A and D as the failures, and exactly
5 passed once the source change lands. Section 9.1's extraction is already free of the diagnostic
scaffolding, so implementation step 1 is a copy with no editing.

### S5.2 Fill in the baselines the plan asks the implementer to record

- **Criterion 2.** The frozen-SHA totals are `Test Files 158 passed (158)`, `Tests 1576 passed (1576)`,
  exit 0. The post-change reading is `159 (159)` and `1581 (1581)`: exactly `+1` file and `+5`
  tests, no existing test regresses. State these numbers in criterion 2 instead of "record the
  frozen-SHA totals before starting", so the implementer cannot record a drifted baseline.
- **Criterion 4.** `dependencies` rises from `1487` to exactly `1493` (`+6`: the file's five
  relative imports plus `vitest`). Replace "the dependency count may rise only by that file's own
  edges" with `1493`, and keep the hard `modules: 343` and `errors: 0`. The module count is
  filesystem-derived, so it reads `343` before `git add -f` as well as after.
- **Criterion 7.** `npm run test:debt` exits 0 at the frozen SHA but prints a non-empty allowlist
  of pre-existing `placeholder-rust-test` entries under `src-tauri/src/phone/mailbox.rs`. Worth a
  half-line, because "exits 0" plus visible allowlist output reads like a failure on first run.

### S5.3 Verdict on delta 1 (owner predicate `(activeSessionId() ?? selectionId())`)

**Agree, and it is load-bearing.** Two mutations, run against the five-case suite with the plan
applied:

- Predicate replaced by bare `activeSessionId() !== id`: **only case A fails.** This reproduces
  section 5.8's first rejected alternative as a measurement rather than an argument, and confirms
  the `?? selectionId()` fallback is what keeps case A's legitimate write alive.
- Guard operator `>` replaced by `>=`: **only case E fails** (see S5.5).

So delta 1 is correct and the A-side of it is covered. The D-side is not, which is S5.4.

### S5.4 The gap this enrichment exists to surface: delta 1's tightening is untested

I mutated the predicate to evidence file 2's original, looser wording (accept whenever the store
is unbound, i.e. `const bound = activeSessionId(); if (bound !== null && bound !== id) return;`).
**All five cases pass.** Nothing in A through E distinguishes the adopted tighter form from the
form it replaced. A later refactor can loosen it back and every gate in section 9.2 stays green.

That matters more than it looks, because the reserve-window behaviour is the *only* thing delta 1
buys over the looser form. Section 10's R2 describes that window accurately but understates it as
"only a sub-frame transient differs"; it is also the entire justification for the delta, and it is
unverified.

I built and ran a falsifier for it. It is case D with the two resolutions swapped: park
`list_sessions`, switch the selection to `SESSION_B`, assert the store is unbound but
`selectionId` is already `SESSION_B`, then resolve session A's save inside that window and assert
it is dropped (`activeWorkgroupTask` stays `null`) before letting B's bind land. Measured: it
**passes** with the plan's predicate and **fails** with the loose one, so it is a real falsifier,
not a tautology. About 45 lines, no new helper, no new import. It is stored verbatim at

```
D:\0_repos\AgentsCommander_iac\.ac\wg-14-dev-v5-team\__agent_dev-webpage-ui\diagnostics\case-F-probe.tsx.txt
```

**Decision for the tech lead, not taken here.** Either (a) admit case F and the suite becomes six,
or (b) keep the A-E ruling and add one explicit sentence to D1 and R2 saying the tightening is
deliberately unverified by the suite and why. What should not happen is the current state, where
D1 reads as a decided improvement and a reader assumes the five cases pin it. I did not add case F
to section 9.1 because the A-E composition is a standing ruling.

### S5.5 Delta 4 (case E) is stronger than the plan claims, and should say so

Section 10's D4 justifies case E only as the falsifier for a never-expiring guard. The `>=`
mutation shows it is also the **only** case that pins the comparison operator: with `>=` the suite
is 4 passed, 1 failed, and E is the failure. An off-by-one here is the most likely single mistake
an implementer can make in section 5.2, and E is the entire net that catches it. Adding that
sentence to D4 raises E from "one extra case" to "the operator gate", which is the argument that
survives a reviewer asking why five and not four.

### S5.6 Verdict on delta 2 (clear `lastLocalTaskWrite` only in `resetForTests`)

**Agree, with a stronger reason than the one recorded.** The plan's self-expiry argument is sound
but is the weaker of the two available, because it depends on the reader accepting the sequence
comparison. The decisive argument is lifecycle, and I verified it by exhaustive `rg` over `src/`
at the frozen SHA:

- `loadLockedSession` has exactly one call site: `src/terminal/App.tsx:270`, inside `onMount`.
- `clearLockedSession` has exactly two: `src/terminal/App.tsx:224` and `:229`, both inside
  `loadLockedSession` itself.

The whole locked-session path therefore runs once per window, at mount, strictly before any save
is reachable: the pencil is disabled until `sessionId()` is non-null (`WorkgroupTask.tsx:92-95`),
so `lastLocalTaskWrite` is provably `null` every time `clearLockedSession` can run. Clearing there
is not merely redundant, it is unreachable. Recommend recording that in D2, so a future reader
does not "restore" the clearing on lifecycle grounds and quietly reintroduce a reset the sequence
comparison was designed to make unnecessary.

Same evidence supports section 7 item 7: the `expectedTaskSeq` parameter on `bindLockedSession` is
provably inert today. Keeping it for symmetry is fine; the plan should keep saying so plainly.

### S5.7 An undocumented backend invariant the guard depends on

The plan declares zero backend coupling, and for edits that is true. For *correctness* it is not
quite: the sequence guard is sound only because "the snapshot was requested after the local write"
implies "the snapshot is at least as fresh". That implication rests on two backend facts at the
frozen SHA:

1. `task_set_title` commits TASK.md to disk before its promise resolves. `task_ops::perform` writes,
   then `emit_task_updated` fires, then `Ok(result)` returns (`src-tauri/src/commands/task.rs:218-243`).
2. `SessionInfo.workgroup_task` is re-read from disk on every list, not cached
   (`src-tauri/src/session/session.rs:396`).

If a later change made `task_set_title` return before the write commits (async write, write-behind
cache, or a debounced flush), the guard would silently stop protecting case A while every test
here stayed green, because the harness mocks the transport. Recommend one line in section 5.1 or
section 7 recording this as a stated dependency on backend behaviour. It costs nothing now and it
is the kind of coupling that is impossible to rediscover from the frontend later.

### S5.8 One residual the plan does not list (suggest R4, do not fix)

`markUnavailable` (`terminal.ts:190`), `safetySuspendDestroyed` (`:196`) and `suspendLiveBinding`
(`:206`) all call `clearLiveMetadata()`, which blanks `activeSessionId`, and none of them touches
`selectionId`. Under delta 1 a local save resolving after any of those is therefore accepted on
the `selectionId` fallback, and paints a title for a session the store has just declared
unavailable.

This is **not** a regression: today's unconditional write does exactly the same thing. And it is
user-visible in both, because `<WorkgroupTask />` (`src/terminal/App.tsx:402`) is gated only on
`!terminalStore.activeIsRootAgent`, not on `shouldMountTerminal()`, so the header keeps rendering
while `bindingState` is `"unavailable"`.

Suggest listing it as R4 alongside R2 rather than closing it. Closing it would mean deciding what
the header should show for an unavailable session, which is a UX question this issue did not ask.

### S5.9 Smaller notes, no action required unless the tech lead wants them

- **Section 3.7's call-graph facts hold.** Re-verified by `rg`: `bindLive` has one caller
  (`App.tsx:120`), `bindLockedSession` one (`App.tsx:222`), and no test calls either. Adding a
  required parameter cannot break an unedited call site.
- **The corrected anchors are right.** `saveTitle`'s store write is `WorkgroupTask.tsx:157` and
  `performClean`'s is `:205` at the frozen SHA; `id` is in scope at both (`:139`, `:189`).
  `selectionId` is `terminal.ts:7` and `activeSessionId` is `:16`, so section 5.3's "no import is
  added" holds.
- **Implementation order step 2's expectation is right in kind.** After step 2 alone the two bind
  call sites pass too few arguments and `tsc` fails there. I applied the three files together, so
  I did not measure the intermediate state; the claim is sound but untested and can stay as a
  narrative aid rather than a gate.
- **Section 9.1's coverage note stands.** No case targets `bindLockedSession`, and per S5.6 that
  state is unreachable, so the deliberate stop at five (or six) cases is correct on that axis.

---

## Step 6 enrichment (dev-rust-grinch)

Author: dev-rust-grinch, wg-14. Appended 2026-08-20 UTC as Step 6 of the Full path. Sections 1
through 11 and the Step 5 enrichment are unmodified. I propose; the architect decides.

Method: I read every cited body at the frozen SHA, then used the ratified materialise-and-revert
to measure instead of argue. Working tree only, no branch, no commit. I extracted section 9.1's
test file, applied 5.2 through 5.7 verbatim, added two probe cases in a separate scratch file,
ran the suite before and after, then probed one alternative owner predicate. Everything was
reverted: `git status --porcelain` is empty and `HEAD` is
`1376c2b84a23125624e919c9af7e65813d624241`. Disclosed in my report to the tech lead.

My pre-change baseline reproduces S5.1 exactly: A FAIL, B PASS, C PASS, D FAIL, E PASS. My
post-change run reproduces 5 passed (5). So everything below sits on top of a confirmed
reproduction, not a disagreement about the measurements.

### G1 (HIGH). `TASK.md` is per-workgroup, so case D's fixture is unreachable and the session-id owner predicate regresses the common switch

**What.** The plan keys ownership of a task write on the **session id**. The thing being written
is not owned by a session. `find_workgroup_task_path_for_cwd` (`src-tauri/src/session/session.rs:243-256`)
walks up from a session's cwd to the first directory whose name starts with `wg-` and returns
`<that dir>/TASK.md`. `read_workgroup_task_for_cwd` (`:258-264`) reads exactly that path, uncached,
on every `SessionInfo::from` (`:396`). **Every session in a workgroup therefore reads one shared
`TASK.md` and always carries identical `workgroupTask`.**

The plan's case D gives two sessions under the same root different task content:

- `SESSION_A`: `C:\Project\.ac\wg-1-dev-team\__agent_architect`, `workgroupTask: OLD_TASK`
- `SESSION_B`: `C:\Project\.ac\wg-1-dev-team\__agent_dev-rust`, `workgroupTask: OTHER_TASK`

Both resolve to `C:\Project\.ac\wg-1-dev-team\TASK.md`. The backend cannot produce that pair. The
test contradicts itself in its own body: it resolves the save with
`{ workgroupRoot: WG_ROOT, task: NEW_TASK }` where `WG_ROOT` is `C:\Project\.ac\wg-1-dev-team`,
which is the very root `SESSION_B` lives under. The fixture originates in the diagnostic harness
(`App.task-clobber-diagnostic.test.tsx:357-359`) and was carried into case D, into case F's probe,
and into section 3.4's instrumented log, so all three inherit it.

**Why (concrete failure scenario, measured).** In AgentsCommander the overwhelmingly common switch
is between agents of the same workgroup, which is what the sidebar lists. Take the reachable form
of case D:

1. Bound to `wg-1-dev-team/architect`. Header shows `Old title`. User saves `New title`;
   `task_set_title` is in flight.
2. User clicks the sibling `wg-1-dev-team/dev-rust`. `reserveSelection(B)` clears metadata,
   `expectedTaskSeq` is captured as 0, `list_sessions` goes in flight.
3. The backend serves that list before `task_set_title` commits, so the sibling's snapshot carries
   the pre-save content of the **shared** file. `bindLive(B, ..., 0)` finds `lastLocalTaskWrite`
   null, applies it. Header shows `Old title`.
4. The save resolves. `applyLocalTask(A, NEW_TASK)` sees `(null ?? SESSION_B) !== SESSION_A` and
   **drops it**. Today's unconditional write would have painted `New title`, which is correct,
   because B displays the same file the user just edited.
5. The header stays on the pre-save title. Per section 3.3 the discovery poll has already committed
   the new content to `task_cache` and will not re-emit, so it stays stale until the next real
   `TASK.md` edit or the next session switch. **This is issue #1455's own symptom, reintroduced by
   the fix, on the path case D claims to fix.**

The `manual` event is not a reliable rescue. `task_set_title` emits it (`src-tauri/src/commands/task.rs:157-172`)
and the handler at `App.tsx:338-348` matches `activeWorkingDirectory` against `workgroupRoot`. During
step 2's window `activeWorkingDirectory` is `""` because `clearLiveMetadata` blanked it, so the
handler returns at `if (!cwd || !workgroupRoot) return;`. Evidence file 1 section 5 already records
that the event is dropped during the in-flight window. Whether it rescues step 5 is a coin flip on
delivery timing, and D3 declines to touch that branch.

The same mis-keying weakens the snapshot side: `localTaskWriteWins(session.id, ...)` will not
suppress a stale snapshot bound to a **sibling** session even though that snapshot shows the same
file the local write just changed.

**Measured, not reasoned.** I added two probe cases beside the plan's five.

- `CASE G`: the step 1-5 sequence above, with the sibling's `workgroupTask` set to `OLD_TASK` (the
  only value the backend can produce), asserting the header ends on `New title`.
- `CASE G2`: the identical sequence but switching to `wg-2-other-team/dev-rust`, which is the only
  reachable form of case D, asserting the save is dropped.

| Tree state | A | B | C | D | E | G | G2 |
|---|---|---|---|---|---|---|---|
| Frozen SHA (no source change) | FAIL | PASS | PASS | FAIL | PASS | **PASS** | FAIL |
| Plan 5.2-5.7 applied verbatim | PASS | PASS | PASS | PASS | PASS | **FAIL** | PASS |

Case G passes today and fails under the plan. That is the definition of a regression, and nothing
in A through F catches it.

**Fix (proposed, architect decides).** Key the owner on the workgroup root, which both call sites
already hold: `TaskUpdateResult.workgroupRoot` (`src-tauri/src/commands/task.rs:25-33`) is returned
by `task_set_title` and `task_clean` alike, so `result.workgroupRoot` is in scope at
`WorkgroupTask.tsx:157` and `:205` with no new IPC, no new command and no new payload field. The
`workgroupRoot`-versus-cwd comparison already exists in this codebase at `App.tsx:342-348`, so the
predicate is a reuse, not an invention. Sketch, deliberately minimal:

```ts
applyLocalTask(id: string, workgroupRoot: string, task: string | null): void {
  const pointsAtWorkgroup = cwdUnderRoot(activeWorkingDirectory(), workgroupRoot);
  if (!pointsAtWorkgroup && (activeSessionId() ?? selectionId()) !== id) return;
  taskWriteSeq += 1;
  lastLocalTaskWrite = { sessionId: id, workgroupRoot, seq: taskWriteSeq };
  setActiveWorkgroupTask(task);
}
```

with `localTaskWriteWins(session.id, session.workingDirectory, expectedTaskSeq)` accepting on
`cwdUnderRoot(cwd, lastLocalTaskWrite.workgroupRoot) || lastLocalTaskWrite.sessionId === sessionId`.
The `(activeSessionId() ?? selectionId()) !== id` term stays as the fallback for the window where
`activeWorkingDirectory` is blank, so delta 1 and case F survive intact.

I measured this shape too. Result: **A, B, C, E, G and G2 all pass, and case D fails.** That is the
point of G1, not a defect in the alternative: case D asserts that a save must not paint into a
sibling's header, and for two sessions sharing one `TASK.md` that assertion is simply false. Case D
is not merely under-specified, it is an incorrect oracle that pins the regression into the plan.

Note I am not claiming the D-side of the fix is worthless. G2 proves the cross-workgroup leak is
real and that the fix closes it. The defect is the ownership key, not the idea of an owner.

### G2 (MEDIUM-HIGH). No acceptance criterion can detect G1, and criterion 1 actively forbids the correct design

Section 9.2 criterion 1 mandates that all five cases pass after the source change. Case D can only
pass if the owner is keyed on session id. So criterion 1 does not merely fail to catch G1, it
**rejects** any implementation that fixes it. An implementer who independently notices the shared
`TASK.md` and keys on the workgroup root is told by the gate that their correct implementation is
wrong and must revert to the regressing one. That is the sharpest possible form of "a green gate
lies", and it is the one thing a reviewer cannot recover from downstream.

The other criteria are blind to it by construction: criterion 3 (`typecheck`) only proves the
parameters were threaded, criterion 4 counts modules and edges, criterion 6 is a file-name list,
criterion 7 looks for skipped tests. Criterion 8 is the only behavioural check and it is explicitly
non-blocking; as written it says "edit a coordinator's TASK title while forcing a reconnect", which
is case A. It never asks the tester to switch to a sibling agent, so it would not surface G1 either.

**Fix (proposed).** Whatever the architect decides on G1, criterion 8 should gain one sentence: after
saving a title, switch to another agent **in the same workgroup** and confirm the header shows the
new title, not the pre-save one. That is a five-second manual check and it is the only one that
exercises the common path.

### G3 (MEDIUM). The security and "cross-session leak" framing is false for the common case

Section 3.4 calls case D "a cross-session data-display leak, not just staleness", and section 8 says
"Case D today leaks one workgroup's TASK content into another session's header; after the fix that
write is dropped." Both are true only when the two sessions are in **different** workgroups. Within
one workgroup there is no leak at all: the two sessions display the same file, and the write the
plan proposes to drop is the correct content for both. Section 8's "the change strictly narrows what
can be displayed" is therefore not accurate either, because on the sibling path it narrows away a
correct value and leaves a stale one.

I raise this separately from G1 because it is the justification a later reader will rely on. If the
architect keeps the session-id key, these two paragraphs still need re-wording, or the plan records
a security benefit it does not have and hides a staleness cost it does.

### G4 (LOW-MEDIUM). The `sessionId` term of `localTaskWriteWins` is untested, and I could not reach it

`localTaskWriteWins` requires `lastLocalTaskWrite.sessionId === sessionId`. Section 7 item 6 states
this as required behaviour ("Snapshot for a different session than the last local write: applied
normally, because the owner does not match"), but no case pins it:

- A, B, C and E involve one session, so the term is always true.
- D and F reject the local write, so `lastLocalTaskWrite` stays `null` and the term is never reached.

I tried to construct a reachable interleaving and could not. To exercise it you need a bind for
session Y whose `expectedTaskSeq` was captured **before** an accepted write for session X. Every
central-window bind is preceded by `reserveSelection`, which publishes the new `selectionId` and so
makes `applyLocalTask` reject the X write; and the only `bindLockedSession` call runs once at mount
before any save is reachable (S5.6). So the term is either dead defensive code or there is a path
neither of us has named.

Not a defect on its own. It matters because under the G1 fix the equivalent term becomes both
reachable and load-bearing, so whichever key the architect picks, section 7 item 6 should either
cite a reachable path or say plainly that it is defensive.

### G5 (LOW). The suite never drives `onSessionSwitched`

Every selection change in A through F is produced by `forceHydration`, which bumps the transport
connection generation and reaches `reconcileSelection` through `applyConnectionState` and
`requestHydration`. A real sidebar click reaches `reconcileSelection(selection, deliveryGeneration,
false)` directly from the `onSessionSwitched` listener (`App.tsx:240-241`) with **no** generation
change. Both paths funnel through `reserveSelection` and `clearLiveMetadata`, so I do not believe
the guard behaves differently and I am not calling this a defect. But the suite should not be
described as covering "a session switch": what it drives is a reconnect that happens to return a
different selection. One sentence in the section 9.1 coverage note would keep a later reader honest.

### G6 (LOW). R4 extension: a save for a *destroyed* session is accepted

Confirming and extending S5.8. `safetySuspendDestroyed` (`terminal.ts:194-203`) clears live metadata
but, when the destroyed id is still the live selection, leaves `selectionId` set to it and moves
`bindingState` to `"pending"`. Under delta 1 a save that resolves after the session was destroyed is
therefore accepted on the `selectionId` fallback and paints a title for a session that no longer
exists, and `<WorkgroupTask />` keeps rendering because `App.tsx:402` gates only on
`activeIsRootAgent`. It also bumps `taskWriteSeq` and arms the guard for a session that can never
bind again.

This is not a regression (today's unconditional write does the same, minus the seq bump) and it
self-heals on the next `markUnavailable`, which clears the task to `null`. I support S5.8's
recommendation to record it as R4 rather than close it, and suggest R4 name `safetySuspendDestroyed`
explicitly alongside `markUnavailable` and `suspendLiveBinding`, because the destroyed case is the
one where "the session the terminal is pointing at" is most clearly a fiction.

### G7 (LOW, suggest R5). The guard is conservative in one direction the plan does not state

S5.7 records the invariant that "requested after the local write" implies "at least as fresh". The
converse is assumed silently and is not true: "requested **before** the local write" does not imply
"stale". A `list_sessions` issued at T0 can read `TASK.md` at T3, and if an external edit committed
at T2 with T0 < T1(local commit) < T2 < T3, the snapshot is genuinely fresher than the local write
but is suppressed anyway, because the ordering token compares request time, not read time.

Consequence: one external `TASK.md` edit can be dropped from the header, and it heals on the next
hydration or session switch. Negligible in size and strictly narrower than the defect being fixed,
so I do not propose changing the design for it. I propose recording it as R5 so that a later reader
does not mistake the seq guard for a freshness guarantee.

### What I tried to break and could not

Stated explicitly so the architect knows where the plan is solid.

- **The seq mechanism itself.** I could not construct an interleaving where the counter fails to
  expire, wraps, or is compared against a token captured in the wrong place. The capture position
  mandated in 5.6 (inside the `try`, immediately above the `await`) is correct, and it is correct
  for the `onSessionSwitched` path as well as the hydration path because both funnel through
  `reconcileSelection`.
- **Overlapping hydrations.** Two `reconcileSelection` calls in flight for different generations:
  the older one is killed by `matchesSelection` before it can bind. Two for the same generation via
  `allowEqualReconnect`: both capture their own token and both behave correctly. Neither can bind
  with a stale token.
- **Concurrent local writes.** `saveTitle` and `performClean` cannot overlap: `saveTitle` sets
  `busy()` synchronously before its `await`, `baseDisabled` folds `busy()` in, and `cleanDisabled`
  additionally folds `editing()`. Local-versus-local therefore stays last-write-wins exactly as
  today, and the seq is assigned in resolve order, so the stamp can never disagree with the value
  actually written.
- **The detached-window path.** `bindLockedSession`'s new parameter is provably always 0.
  `loadLockedSession` has one caller inside `onMount`, `clearLockedSession` has two, both inside
  `loadLockedSession` itself, and the pencil is disabled until `activeSessionId` is non-null. The
  module-scope tokens are per-webview, so a detached window starts at 0 and cannot be armed before
  its only bind. S5.6 is correct.
- **SolidJS reactivity.** `taskWriteSeq` as a plain getter on a store object of otherwise reactive
  getters is a footgun, but not a live one: it is read only from `reconcileSelection` and
  `loadLockedSession`, both after an `await` or from an untracked event handler, so no subscription
  is created and no memo is silently frozen. `applyLocalTask`'s reads of `activeSessionId()` and
  `selectionId()` are the same pattern as the existing `renameBoundSession` and
  `safetySuspendDestroyed`, so it adds no new class of hazard. The decision in 5.2 not to make it a
  signal is right.
- **Reset and test isolation.** `resetForTests` is reached from `resetUiStoresForTests`
  (`ui-harness.tsx:291`), so the two extra reset lines in 5.5 do close the module-scope leak between
  test files. D2 holds.
- **Call-graph blast radius.** Re-verified independently: `bindLive` has one caller (`App.tsx:120`),
  `bindLockedSession` one (`App.tsx:222`), and no test file calls either. Adding a required
  parameter cannot break an unedited call site. Section 3.7 and S5.9 are correct.
- **Typecheck.** `npx tsc --noEmit` exits 0 with 5.2 through 5.7 applied verbatim, and also exits 0
  with the G1 alternative applied, so neither shape has a typing cost.
- **Section 11's cycle statement.** No new import is added by either shape; the G1 fix adds two
  module-local functions to `terminal.ts` and one extra argument, so the zero-new-arcs claim holds
  for the alternative as well.

### Summary of what I ask the architect to rule on

1. **G1.** Re-key the owner on the workgroup root and correct case D, or record explicitly that the
   plan knowingly trades a same-workgroup staleness regression for the cross-workgroup fix.
2. **G2.** If case D stands as written, say in criterion 1 that it forbids the workgroup-root design
   on purpose, so the implementer does not think they found a bug. Either way, extend criterion 8
   with the sibling-switch check.
3. **G3.** Re-word sections 3.4 and 8 so the leak and the security narrowing are scoped to
   cross-workgroup switches.
4. **G4, G5, G6, G7.** Documentation only: cite or disclaim section 7 item 6; say the suite drives a
   reconnect rather than a switch; extend R4 to name `safetySuspendDestroyed`; add R5.

I take no position on case F, which is the tech lead's standing ruling. I note only that its fixture
inherits the same impossible pair as case D, so whatever is decided about D's fixture applies to F's.

---

## Step 7 consensus resolution (architect, round 1)

Method: the same materialise-and-revert the two enrichers used. I extracted section 9.1's suite into `src/terminal/App.workgroup-task-race.test.tsx`, applied sections 5.2 through 5.7, ran the gates, then applied the two refuted owner keys as mutations and re-ran. Everything was reverted: `git status --porcelain` is empty and `HEAD` is `1376c2b84a23125624e919c9af7e65813d624241`.

I measured rather than predicted because finding G1's lesson is precisely that an unmeasured assumption can pin a regression into a green gate, and because the design I certified is not the one Step 6 measured: it drops the session-identity fallback that the G1 sketch kept.

**Suite behaviour by tree state:**

| Tree state | A | B | C | D | E | F | G | H | Totals |
|---|---|---|---|---|---|---|---|---|---|
| Frozen SHA, no source change | FAIL | PASS | PASS | FAIL | PASS | **PASS** | FAIL | PASS | 3 failed, 5 passed |
| Step 4 draft (session-id key) | PASS | PASS | PASS | PASS | PASS | **FAIL** | **FAIL** | PASS | 2 failed, 6 passed |
| This plan (workgroup-root key) | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | 8 passed |

Reading the F column top to bottom is the whole of finding G1: F passes today and fails under the Step 4 draft, which is the definition of a regression. The G column is the additional hole this plan closes and the Step 6 sketch does not: with a session-identity fallback in the write path, a sibling's save is rejected inside the unbound window and the pre-save snapshot then binds.

**Gates with this plan applied:**

| Gate | Frozen SHA | This plan |
|---|---|---|
| `npx tsc --noEmit` | 0 | 0 |
| `npx vitest run src/terminal/App.workgroup-task-race.test.tsx` | 3 failed, 5 passed (8) | 8 passed (8) |
| `npm test` | `158 (158)` files, `1576 (1576)` tests | `159 (159)` files, `1584 (1584)` tests |
| `npm run check:frontend-dependencies` | `modules: 342, errors: 0, dependencies: 1487`, OK | `modules: 343, errors: 0, dependencies: 1493`, OK |
| `npm run test:debt` | exit 0 | exit 0 |
| `git diff --stat -- src/` | clean | `App.tsx +9/-3`, `WorkgroupTask.tsx +2/-2`, `terminal.ts +88/-3` |

The `npm test` frozen-SHA baseline reproduces Step 5's S5.1 measurement exactly (`158`/`1576`), so all three passes agree on the starting point.

**Disposition of every Step 5 and Step 6 item:**

| Item | Ruling |
|---|---|
| G1 (per-workgroup TASK.md, wrong owner key) | **Accepted and extended.** Re-keyed on the workgroup root (D1), and the session-identity fallback the sketch kept was dropped too (D2), because it leaves case G broken. Sections 3.2, 3.5, 5.1, 5.3, 5.9 rewritten. |
| G2 (criteria cannot detect G1; criterion 1 forbids the fix) | **Accepted.** Criterion 1 now names the exact three-failure shape including F passing; criterion 3 is a new mutation probe that fails the certification if the suite stops detecting the wrong key; criterion 10b adds the sibling-switch manual check. |
| G3 (leak and security wording false within a workgroup) | **Accepted.** Section 3.5 now separates the two defects by axis, and section 8 scopes the confidentiality claim to cross-workgroup and explicitly disclaims "strictly narrows". |
| G4 (`sessionId` term unreachable) | **Resolved, not documented.** The re-key replaces that term with one that cases G and H reach in both directions. Section 10 records it. |
| G5 (suite drives a reconnect, not a sidebar click) | **Accepted.** Section 9.1's coverage note now says so. |
| G6 (R4 should name `safetySuspendDestroyed`) | **Accepted.** R4 names it and explains why it is the sharpest form. |
| G7 (request time is not read time) | **Accepted** as R5. |
| S5.1, S5.2 (baselines) | **Accepted**, re-measured for the eight-case suite. `npm test` is `159`/`1584`, dependencies `1493`. Criterion 9 records the expected `test:debt` allowlist output. |
| S5.3, S5.4, case F probe | **Superseded.** See D7: the predicate those pinned no longer exists; cases G and H cover the window instead. |
| S5.5 (case E is the operator gate) | **Accepted** as D6. |
| S5.6 (D2's stronger lifecycle reason) | **Accepted**, now the decisive reason in section 5.5, recorded as D3. |
| S5.7 (undocumented backend dependency) | **Accepted** and enlarged into section 5.8, which now carries three facts including the per-workgroup one G1 turned on. |
| S5.8 (suggest R4) | **Accepted** as R4, merged with G6. |
| S5.9 note 3 (implementation order step 2 intermediate state untested) | **Accepted as narrative.** Section 12 step 2 now says explicitly that the intermediate failure is expected and is not measured as a gate. |
| S5.9 note 4 (coverage note stands) | **Accepted.** The `bindLockedSession` non-coverage is still deliberate and still stated, in section 7 item 8 and the 9.1 coverage note. |
| S5.9 notes 1 and 2 (call-graph and anchors re-verified) | **Accepted**, folded into section 3.8. |

**Certification.** No open decision remains. The dependency-cycle gate (section 11) passes, measured. Verdict: `READY_FOR_IMPLEMENTATION`.
