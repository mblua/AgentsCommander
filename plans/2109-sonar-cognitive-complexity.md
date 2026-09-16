# Plan #2109: Clear five `typescript:S3776` cognitive-complexity findings

Status: READY_FOR_IMPLEMENTATION

- Issue: [mblua/AgentsCommander#2109](https://github.com/mblua/AgentsCommander/issues/2109)
- Repo: `repo-AgentsCommander`; branch `fix/2109-sonar-cognitive-complexity`; base `main` = `0cb058a5`.
- Class: Lite (band 1-25). Owner `ac-dev-webpage-ui-v4`; reviewer Grinch; coordinator `ac-tech-lead-v4`.
- Rule `typescript:S3776`: a function fails when its Cognitive Complexity exceeds 15. Requirement: zero behavior change.

## 1. Objective

Refactor the five functions below so each lands at most 15, with no product behavior change. All five are code moves into named functions, not logic rewrites.

| # | Issue key (SonarCloud) | File:line at base | Function | Now | Target |
|---|---|---|---|---|---|
| 1 | `AaBBZrqobbRnCQnTRHfp` | `src/sidebar/stores/team-idle-watcher.ts:89` | `createEffect` callback inside `startTeamIdleWatcher` | 68 | 5 |
| 2 | `AaBBZrknbbRnCQnTRHby` | `src/sidebar/components/ProjectPanel.tsx:4229` | delete-room `onClick` arrow | 17 | 5 |
| 3 | `AaBBZrfvbbRnCQnTRHXe` | `src/main/components/ErrorModal.tsx:40` | second `onMount` `onKeyDown` | 19 | 3 |
| 4 | `AaBBZrgEbbRnCQnTRHXk` | `src/main/components/QuitConfirmModal.tsx:20` | `onMount` `onKeyDown` | 22 | 6 |
| 5 | `AaBBZryFbbRnCQnTRHhG` | `src/terminal/components/TaskCleanConfirmModal.tsx:17` | `onMount` `onKeyDown` | 19 | 6 |

The "Now" column comes from the SonarCloud issue messages at base; the "Target" column is the sum of the same Sonar increments after the moves, itemized in §7.

## 2. Evidence at base

- F1. The five keys, files, lines, messages and per-line "flows" (Sonar's own increment list, each tagged `+1` or `+N (incl. M for nesting)`) are public on the SonarCloud web API, no token needed for this public project. The flow sums equal the message number in every case (verified: 68, 17, 19, 22, 19).
- F2. `team-idle-watcher.ts` has one `createRoot`, one `createEffect` at line 89, and two already-extracted, already-tested pure helpers: `shouldSuppressBeep` (20) and `updateGraceOnFocusChange` (30). The effect registers session→room mappings (`sessionToWg` grows monotonically), snapshots busy state, seeds `previousByWg` on the first run, arms focus grace, beeps on busy→idle transitions, prunes grace, then replaces the snapshot.
- F3. `src/sidebar/stores/team-idle-watcher.test.ts` unit-tests only `shouldSuppressBeep` and `updateGraceOnFocusChange`; its header comment states the effect wiring is audit/review, not mocked. It already runs under jsdom with a `MockWebSocket`. This plan adds the first integration case and updates that header comment to point at it.
- F4. `ProjectPanel.tsx:4229` is the inline `onClick={async () => { … }}` of the Delete button in the delete-room modal. Its `catch` maps a `delete_workgroup` failure into modal state (`BLOCKERS:` payload, `DIRTY_REPOS:` payload when not forcing, or the raw message). `retryWgDelete` at line 861 holds a near-duplicate with five observable differences (progress signal, force-independent `DIRTY_REPOS`, extra `setWgBlockers(null)` resets, different parse-failure text, no `setWgLastForceUsed`).
- F4b. Two further S3776 findings exist at base in the same file and are pre-existing, outside #2109 and claimed elsewhere: `AaBBZrkmbbRnCQnTRHZx` (`ProjectPanel.tsx:270`, `normalizePath`, 16, OPEN since 2026-06-30) and `AaBBZrkmbbRnCQnTRHZ_` (`ProjectPanel.tsx:861`, `retryWgDelete`, 16, OPEN since 2026-05-02). They survive this PR by design; success is the five #2109 keys CLOSED, not zero S3776 rows in a touched file.
- F5. `src/sidebar/components/ProjectPanel.delete-diagnostics.workflow.test.tsx` drives the 4229 handler end to end with `FakeTransport` through the `BLOCKERS:`→Retry and `DIRTY_REPOS:`→force paths.
- F6. The three modals each register a capture-phase `keydown` listener from `onMount` and have no test file. `QuitConfirmModal` and `TaskCleanConfirmModal` route Escape and Enter identically apart from the button/callback pair; `ErrorModal` additionally guards on `errorModalStore.open`, has no Enter branch, calls `stopImmediatePropagation()` before the Tab focus logic, and calls `stopImmediatePropagation()` for every other key.
- F7. The Tab-wrap block (`focusables.length < 2`, `indexOf`, `idx === -1`, `shiftKey` ternary, shift/else wrap) is byte-identical in all three modals and accounts for 13-16 of their increments.
- F8. Root `.gitignore:11` ignores `/plans/`; commit this plan with `git add -f`.
- F9. Verification is by re-querying the SonarCloud API after the branch/PR analysis (§7.3); Sonar is the sole authority for cell values, so no local complexity reimplementation is added.

## 3. Decided refactor per function

### 3.1 `team-idle-watcher.ts` — extract 7 module-level functions (68 → 5)

Add `import type { ProjectState } from "./project";` (type-only; `./project` is already imported for `projectStore`). `shouldSuppressBeep`, `updateGraceOnFocusChange`, `startOsFocusListener`, `isExited`, `isBusy` are unchanged.

```ts
function registerSessionsForProjects(
  projects: readonly ProjectState[],
  sessionToWg: Map<string, string>,
  findSessionByName: (name: string) => Session | undefined,
): void {
  for (const project of projects) {
    for (const wg of project.workgroups) {
      for (const replica of wg.agents) {
        const session = findSessionByName(`${wg.name}/${replica.name}`);
        if (session && !sessionToWg.has(session.id)) {
          sessionToWg.set(session.id, wg.path);
        }
      }
    }
  }
}

function collectBusyByWg(
  sessionToWg: ReadonlyMap<string, string>,
  sessionsById: ReadonlyMap<string, Session>,
): Map<string, Map<string, boolean>> {
  const currentByWg = new Map<string, Map<string, boolean>>();
  for (const [sessionId, wgPath] of sessionToWg) {
    const session = sessionsById.get(sessionId);
    if (!session) continue;
    if (isExited(session.status)) continue;
    let inner = currentByWg.get(wgPath);
    if (!inner) {
      inner = new Map<string, boolean>();
      currentByWg.set(wgPath, inner);
    }
    inner.set(sessionId, isBusy(session));
  }
  return currentByWg;
}

export function hasBusyToIdleTransition(
  previousBusy: ReadonlyMap<string, boolean>,
  currentBusy: ReadonlyMap<string, boolean>,
): boolean {
  for (const [sessionId, wasBusy] of previousBusy) {
    if (!wasBusy) continue;
    if (currentBusy.get(sessionId) === false) return true;
  }
  return false;
}

export function allSessionsIdle(currentBusy: ReadonlyMap<string, boolean>): boolean {
  if (currentBusy.size === 0) return false;
  for (const isBusyNow of currentBusy.values()) {
    if (isBusyNow) return false;
  }
  return true;
}

export function beepIdleTransitions(
  currentByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
  previousByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
  focusedWg: string | null,
  graceUntil: ReadonlyMap<string, number>,
  now: number,
): void {
  for (const [wgPath, currentBusy] of currentByWg) {
    const previousBusy = previousByWg.get(wgPath);
    if (!previousBusy) continue;
    if (!hasBusyToIdleTransition(previousBusy, currentBusy)) continue;
    if (!allSessionsIdle(currentBusy)) continue;
    if (shouldSuppressBeep(wgPath, focusedWg, graceUntil, now)) continue;
    void playTeamIdleBeep();
  }
}

export function pruneExpiredGrace(graceUntil: Map<string, number>, now: number): void {
  for (const [wgPath, until] of graceUntil) {
    if (now >= until) graceUntil.delete(wgPath);
  }
}

function replacePreviousByWg(
  previousByWg: Map<string, Map<string, boolean>>,
  currentByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
): void {
  previousByWg.clear();
  for (const [wgPath, perSession] of currentByWg) {
    previousByWg.set(wgPath, new Map(perSession));
  }
}
```

The effect callback becomes exactly:

```ts
createEffect(() => {
  const sessions = sessionsStore.sessions;
  const projects = projectStore.projects;
  const enabled = settingsStore.current?.teamIdleBeepEnabled ?? true;
  const activeId = sessionsStore.activeId;
  const hasOsFocus = osFocused();

  registerSessionsForProjects(projects, sessionToWg, (name) =>
    sessionsStore.findSessionByName(name),
  );

  const sessionsById = new Map<string, Session>();
  for (const s of sessions) sessionsById.set(s.id, s);

  const currentByWg = collectBusyByWg(sessionToWg, sessionsById);

  const focusedWg =
    hasOsFocus && activeId ? sessionToWg.get(activeId) ?? null : null;

  if (!initialized) {
    initialized = true;
    replacePreviousByWg(previousByWg, currentByWg);
    return;
  }

  previousFocusedWg = updateGraceOnFocusChange(
    previousFocusedWg,
    focusedWg,
    graceUntil,
    Date.now(),
    GRACE_MS,
  );

  if (enabled) {
    const now = Date.now();
    beepIdleTransitions(currentByWg, previousByWg, focusedWg, graceUntil, now);
    pruneExpiredGrace(graceUntil, now);
  }

  replacePreviousByWg(previousByWg, currentByWg);
});
```

Reactive reads (`sessionsStore.sessions`, `projectStore.projects`, `settingsStore.current`, `sessionsStore.activeId`, `osFocused()`) keep their original order at the top of the callback. `findSessionByName` is still called synchronously from the effect (inside the arrow evaluated while `registerSessionsForProjects` runs), so SolidJS keeps tracking it.

Two helpers are restructured, not moved verbatim, but with provably identical results:

- `hasBusyToIdleTransition` replaces the `hadTransition` flag loop: `previousBusy` was busy and `currentBusy.get(id) === false` is exactly the old `if (!wasBusy) continue` + `if (isBusyNow === false) { hadTransition = true; break; }`.
- `allSessionsIdle` replaces `let allIdle = currentBusy.size > 0; if (allIdle) { for(...) if (busy) allIdle = false; }`: empty → false, any busy → false, otherwise true, which is the old `allIdle` value in all three cases.

`replacePreviousByWg` clears then copies. The init branch previously copied into an empty map without clearing; clearing an empty map is a no-op, so the branch is identical.

### 3.2 `ProjectPanel.tsx` — extract the failure handler (17 → 5)

Insert next to `retryWgDelete` (before line 861), inside the same component scope that owns `retryGen` and the setters:

```ts
const applyWgDeleteFailure = (e: any, forceDelete: boolean, myGen: number) => {
  if (myGen !== retryGen) return;
  console.error("delete_workgroup failed:", e);
  const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete room";
  if (msg.startsWith("BLOCKERS:")) {
    try {
      const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
      setWgBlockers(report);
      setWgDirtyRepos(false);
      setWgConfirmText("");
      setWgDeleteError("");
      setWgDeleteInProgress(false);
      return;
    } catch (parseErr) {
      console.error("Failed to parse BLOCKERS: payload:", parseErr);
      setWgDeleteError("Room is locked, but the blocker report could not be parsed. Try again.");
      setWgDeleteInProgress(false);
      return;
    }
  }
  if (!forceDelete && msg.startsWith("DIRTY_REPOS:")) {
    setWgDeleteError(msg.slice("DIRTY_REPOS:".length));
    setWgDirtyRepos(true);
    setWgConfirmText("");
    setWgDeleteInProgress(false);
    return;
  }
  setWgDeleteError(msg);
  setWgDeleteInProgress(false);
};
```

The 4229 `onClick` keeps its guard, generation, force and success path; only its `catch` body is replaced:

```tsx
} catch (e: any) {
  applyWgDeleteFailure(e, forceDelete, myGen);
  return;
}
closeWgDeleteModal();
```

Every setter call, message string, console call, ordering and early return is moved verbatim; `retryWgDelete` is untouched.

### 3.3 Modals — shared Tab trap (19 → 3, 22 → 6, 19 → 6)

New file `src/shared/focus-trap.ts`:

```ts
/**
 * Keeps Tab / Shift+Tab focus inside a modal's focusable set: wraps from the
 * last element to the first (or first to last) and calls preventDefault only
 * when it moves focus itself. Callers own the `e.key === "Tab"` check because
 * each modal has its own propagation rules.
 */
export function trapTabFocus(e: KeyboardEvent, focusables: readonly HTMLElement[]): void {
  if (focusables.length < 2) return;
  const idx = focusables.indexOf(document.activeElement as HTMLElement);
  if (idx === -1) {
    e.preventDefault();
    (e.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
    return;
  }
  if (!e.shiftKey) {
    if (idx === focusables.length - 1) {
      e.preventDefault();
      focusables[0].focus();
    }
    return;
  }
  if (idx <= 0) {
    e.preventDefault();
    focusables[focusables.length - 1].focus();
  }
}
```

All three files add `import { trapTabFocus } from "../../shared/focus-trap";` and replace only the Tab inner block with one call; Escape/Enter routing, propagation calls and their order are untouched.

- `ErrorModal.tsx:40` becomes:

```ts
const onKeyDown = (e: KeyboardEvent) => {
  if (!errorModalStore.open) return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    errorModalStore.dismissCurrent();
    return;
  }
  if (e.key === "Tab") {
    e.stopImmediatePropagation();
    trapTabFocus(e, [messageRef, copyBtnRef, dismissBtnRef].filter(Boolean) as HTMLElement[]);
    return;
  }
  e.stopImmediatePropagation();
};
```

- `QuitConfirmModal.tsx:20` becomes:

```ts
const onKeyDown = (e: KeyboardEvent) => {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    props.onCancel();
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault();
    e.stopPropagation();
    if (document.activeElement === quitBtnRef) {
      props.onQuit();
    } else {
      props.onCancel();
    }
    return;
  }
  if (e.key === "Tab") {
    trapTabFocus(e, [cancelBtnRef, quitBtnRef].filter(Boolean) as HTMLElement[]);
  }
};
```

- `TaskCleanConfirmModal.tsx:17` is the same shape with `confirmBtnRef`/`props.onConfirm`.

The helper's branch semantics are identical to the replaced code: fewer than two focusables → no-op; active element outside the set → focus first (last with Shift), with `preventDefault`; on the last element with plain Tab or the first with Shift+Tab → wrap and `preventDefault`; otherwise leave the event alone.

## 4. Behavior and edge cases that must stay identical

- SolidJS reactivity: the five reactive reads stay in the effect callback in the original order; helper calls are synchronous within that callback; no reactive read is moved inside a promise, timeout or event handler. `sessionToWg` still only gains entries. Effect-run ordering (register → build maps → focused WG → init branch → grace → beep → prune → snapshot) is preserved.
- Init branch: first run seeds the snapshot and returns before `updateGraceOnFocusChange`, as today.
- Grace boundary: `shouldSuppressBeep`'s half-open window (`now < until`) is unchanged and covered by existing tests.
- Beep decision: `previousByWg` missing → skip; no busy→idle transition → skip; not all sessions idle (including empty) → skip; focused WG or active grace → skip; otherwise beep. Identical to the old loop order.
- `ProjectPanel`: the two generation checks, the `wgDeleteInProgress`/`activeReplicas` guards, `setWgLastForceUsed`, `forceDelete` capture, and `closeWgDeleteModal()` on success are unchanged. `BLOCKERS:` always wins over `DIRTY_REPOS:`; `DIRTY_REPOS:` only downgrades to an error when forcing. Setters run in the same order and the unparsable-BLOCKERS path keeps its distinct message.
- Modals: key routing, `preventDefault`, `stopPropagation` vs `stopImmediatePropagation`, the ErrorModal `open` guard, the Tab `stopImmediatePropagation` before the length check, and the ErrorModal catch-all `stopImmediatePropagation` are preserved. Focus lists are built at keydown time exactly as before.

## 5. Files touched

| File | Change |
|---|---|
| `src/sidebar/stores/team-idle-watcher.ts` | Extract 7 module-level helpers; rewrite the effect body |
| `src/sidebar/components/ProjectPanel.tsx` | Add `applyWgDeleteFailure`; call it from the 4229 catch |
| `src/main/components/ErrorModal.tsx` | Use `trapTabFocus` for the Tab branch |
| `src/main/components/QuitConfirmModal.tsx` | Same |
| `src/terminal/components/TaskCleanConfirmModal.tsx` | Same |
| `src/shared/focus-trap.ts` | New shared helper |
| `src/sidebar/stores/team-idle-watcher.test.ts` | Cases for the new decision helpers plus one `startTeamIdleWatcher` wiring integration case; header comment updated |
| `src/sidebar/components/ProjectPanel.delete-diagnostics.workflow.test.tsx` | Cases for the generic and unparsable-BLOCKERS failures |
| `src/shared/focus-trap.test.ts` | New helper unit tests |
| `src/main/components/ErrorModal.test.tsx` | New key-routing tests |
| `src/main/components/QuitConfirmModal.test.tsx` | New key-routing tests |
| `src/terminal/components/TaskCleanConfirmModal.test.tsx` | New key-routing tests |

No `src-tauri/`, IPC, transport, CSS, or dependency change.

## 6. Tests

Existing tests must pass with assertions unchanged:

- `team-idle-watcher.test.ts` (both helper suites).
- `ProjectPanel.delete-diagnostics.workflow.test.tsx` (BLOCKERS→Retry and DIRTY_REPOS→force).

New cases:

1. `team-idle-watcher.test.ts` pure-helper cases (mock `../../shared/sound` with `vi.mock` so no audio): `hasBusyToIdleTransition` busy→idle true, busy→busy false, idle→idle false, absent key false; `allSessionsIdle` empty false, all idle true, any busy false; `beepIdleTransitions` beeps once on transition + all idle, skips the focused WG, skips inside grace, skips while a session is still busy, skips unknown WG; `pruneExpiredGrace` deletes only expired entries.
2. `team-idle-watcher.test.ts` wiring integration case (the R3 proof): one `it` starts the real `startTeamIdleWatcher()` against the real stores and a `FakeTransport` (`__setTransportForTests` from `../../shared/ipc`), with `@tauri-apps/api/window` mocked to throw so `osFocused` stays true and `../../shared/sound` mocked to a spy. Setup: resolve `new_project` and `discover_project` for a `wg-1-dev-team` workgroup with agent `architect` (matching the harness `session()` name), resolve `get_settings` with `baseSettings({ teamIdleBeepEnabled: true })` (the harness default is false), `await projectStore.createAndLoad("C:\\Project")`, `await settingsStore.load()`, `sessionsStore.setSessions([session()])` (busy: default `waitingForInput: false`). Phases in order: (a) `await hop()` → beep count 0 (first tick seeds, never beeps); (b) `sessionsStore.setSessionWaiting("session-1", true)` → `waitFor` beep count 1 (busy→idle beeps once); (c) reload settings with `teamIdleBeepEnabled: false`, `setSessionWaiting(false)` then `(true)` → count stays 1 (disabled gate); (d) reload settings with `teamIdleBeepEnabled: true`, `sessionsStore.setVisibleActiveIdForTests("session-1")`, busy→idle again → count stays 1 (focused WG suppressed). `dispose()` and transport restore in `finally`.
3. `ProjectPanel.delete-diagnostics.workflow.test.tsx`: a generic `delete_workgroup` throw shows the raw message and no blocker panel; `BLOCKERS:not-json` shows the unparsable-report message.
4. `focus-trap.test.ts`: fewer than two focusables → no default prevented and focus unchanged; active element outside the set → first (Shift: last) focused with default prevented; Tab on last and Shift+Tab on first wrap with default prevented; mid-list Tab/Shift+Tab leave the event untouched.
5. `QuitConfirmModal.test.tsx` / `TaskCleanConfirmModal.test.tsx` (`render` from `solid-js/web`): Escape calls cancel; Enter on the destructive button calls quit/confirm and on any other element calls cancel; Tab from the last button wraps to the first. `ErrorModal.test.tsx`: enqueue an entry, Escape dismisses; while closed a keydown is not intercepted; while open, a second document listener registered after mount does not receive a Tab or unrelated key (locks in `stopImmediatePropagation`) and Tab wraps focus. Reset the store with `__resetErrorModalStoreForTests()` between cases.

Every new test file starts with `// @vitest-environment jsdom`; `vitest.config.ts:12` defaults to node, so the pragma is required for the focus-trap and modal tests. The integration case runs in the already-jsdom `team-idle-watcher.test.ts`.

Review gate (named, in addition to the integration case): confirm the diff does not (1) drop the `enabled` gate, (2) move `replacePreviousByWg` before `beepIdleTransitions`, (3) swap `pruneExpiredGrace` before `beepIdleTransitions`, or (4) remove the `if (!initialized)` early return.

## 7. Verification

### 7.1 Increment accounting (Sonar's own per-line flows, moved)

`team-idle-watcher.ts` original increments and their new homes:

| Original line:increment | New function | Contribution |
|---|---|---|
| 96:+1, 97:+2, 98:+3, 102:+4, 102:+1 | `registerSessionsForProjects` | 11 |
| 113:+1, 115:+2, 116:+2, 118:+2 | `collectBusyByWg` | 7 |
| 151, 152, 154 | `hasBusyToIdleTransition` (restructured) | 1+2+2 = 5 |
| 162, 163, 164 | `allSessionsIdle` (restructured) | 1+1+2 = 4 |
| 146:+2→+1, 148:+3→+2, 159:+3→+2, 170:+3→+2, 172:+3→+2 | `beepIdleTransitions` | 1+2+2+2+2 = 9 |
| 177:+2→+1, 178:+3→+2 | `pruneExpiredGrace` | 3 |
| 183:+1 | `replacePreviousByWg` | 1 |
| 110:+1, 126:+1+1, 128:+1, 130 removed, 144:+1 | effect callback | 5 |

Reduced counts are nesting resets (moved out of the effect) plus the two flagged restructures; every remaining function is at most 11. Total moved work: 45 increments.

`ProjectPanel.tsx`: effect callback keeps 4230:+1, 4231:+1, 4239:+1, 4241:+1, catch:+1 = 5. `applyWgDeleteFailure` takes 4243, 4245, 4246, 4255, 4262 with nesting reset: 1+1+1+2+2 = 7.

Modals: handlers keep 41/42/48 (ErrorModal) = 3, 21/27/30/32/37 (Quit) = 6, 18/24/27/29/34 (TaskClean) = 6. `trapTabFocus` restructures the wrap with nesting reset: 1 (`length<2`) + 1 (`idx===-1`) + 2 (ternary) + 1 (`!shiftKey`) + 2 (last→first) + 1 (`idx<=0`) = 8.

### 7.2 Commands

```bash
npm run typecheck
npx vitest run src/sidebar/stores/team-idle-watcher.test.ts src/shared/focus-trap.test.ts src/main/components/ErrorModal.test.tsx src/main/components/QuitConfirmModal.test.tsx src/terminal/components/TaskCleanConfirmModal.test.tsx src/sidebar/components/ProjectPanel.delete-diagnostics.workflow.test.tsx
npm run test:debt
npm test
```

### 7.3 SonarCloud acceptance (authority)

Success is exactly the five #2109 keys CLOSED. The two pre-existing ProjectPanel findings in F4b are out of scope and stay OPEN; their presence is not a failure, and no query may be scoped by file (a file-scoped query would conflate them with this PR).

1. PR-scoped query: no S3776 issue attributable to this PR's changed/new lines.

```bash
curl -s "https://sonarcloud.io/api/issues/search?componentKeys=mblua_AgentsCommander&rules=typescript:S3776&resolved=false&pullRequest=<PR_NUMBER>&ps=100" \
  | jq -r '.issues[] | [.key, .component, .line, .message] | @tsv'
```

Expected: none of the five #2109 keys. SonarCloud PR-scoped queries are new-code-only, so the two F4b keys are not expected in this list either; absence here is necessary but not sufficient, hence step 2.

2. Explicit per-key status (the acceptance signal). While the PR is open, the five keys are still OPEN in the `main` context; after merge and the next `main` analysis they must be CLOSED. Query all five by key, optionally also with the branch name for the branch analysis context:

```bash
for KEY in AaBBZrqobbRnCQnTRHfp AaBBZrknbbRnCQnTRHby AaBBZrfvbbRnCQnTRHXe AaBBZrgEbbRnCQnTRHXk AaBBZryFbbRnCQnTRHhG; do
  curl -s "https://sonarcloud.io/api/issues/search?issues=$KEY" \
    | jq -r '"\(.issues[0].key) \(.issues[0].status) \(.issues[0].resolution)"'
done
```

Expected after merge: five lines ending `CLOSED FIXED`. The F4b keys (`AaBBZrkmbbRnCQnTRHZx`, `AaBBZrkmbbRnCQnTRHZ_`) are not in this list and are expected to remain `OPEN`.

### 7.4 Diff check

```bash
git diff --stat 0cb058a5..HEAD
```

Only the files in §5 changed, with no whitespace-only churn.

## 8. Acceptance criteria

- AC1. The five #2109 keys (`AaBBZrqobbRnCQnTRHfp`, `AaBBZrknbbRnCQnTRHby`, `AaBBZrfvbbRnCQnTRHXe`, `AaBBZrgEbbRnCQnTRHXk`, `AaBBZryFbbRnCQnTRHhG`) are CLOSED after merge per §7.3.2; no S3776 issue is attributable to this PR's changed lines per §7.3.1. The two pre-existing ProjectPanel keys (F4b) remain OPEN and are explicitly out of scope.
- AC2. Each refactored function's Sonar complexity is at most 15; the accounting in §7.1 predicts 5, 5, 3, 6, 6 for the five issue functions and 8/11/7/5/4/9/3/1 for the new helpers.
- AC3. All existing tests pass with assertions unchanged; new tests in §6 pass; `npm run typecheck`, `npm run test:debt` and `npm test` are green.
- AC4. Diff limited to §5; no comment, copy, styling, IPC, or dependency change.
- AC5. No behavior change: reactivity, ordering, grace/beep semantics, delete-error mapping and modal key propagation are exactly as at base.

## 9. Rejected alternatives

- One shared modal `keydown` factory: rejected. The three modals differ in propagation (`stopImmediatePropagation` vs `stopPropagation`, Tab stop position, ErrorModal's catch-all) and in Enter handling; unifying key routing would change event semantics, which the zero-behavior-change requirement forbids. Only the Tab trap (F7) is shared.
- Reusing the new `applyWgDeleteFailure` inside `retryWgDelete`: rejected. The five differences in F4 are observable (which progress signal spins, whether a forced retry accepts `DIRTY_REPOS`, extra blocker resets, two user-visible strings), and that function is itself a pre-existing S3776 finding claimed elsewhere (F4b), so reworking it here would collide with that claim and widen the diff.
- Injecting the beep callback into `beepIdleTransitions` for tests: rejected; tests use `vi.mock("../../shared/sound")`, the pattern already used in the suite, and production keeps calling `playTeamIdleBeep` directly.
- Extracting the modals' focus lists or refs into a helper: rejected; no complexity benefit, and it would move ref lifecycles away from the components.
