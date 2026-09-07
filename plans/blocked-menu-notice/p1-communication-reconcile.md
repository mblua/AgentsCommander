# P1: reconcile `communication` from the listing the app already polls

Class: patterned. Owner: frontend. Depends on: nothing.
Status: READY_FOR_IMPLEMENTATION

## Objective

The sidebar's view of `session.communication` is fed only by the `session_communication_changed`
event. Every way that event can be missed (a mutual clobber in the backend, a dropped event, a
transport reconnect, a window reload) leaves the sidebar permanently wrong, because the backend does
not republish while the terminal screen is quiet.

The fix costs almost nothing: `refreshProfileOutdated` already polls `SessionAPI.list()` every five
seconds, that listing already carries `communication`, and the code discards it. Start using it.

## Exact files

1. `src/sidebar/stores/sessions.ts`
2. `src/sidebar/App.tsx`
3. `src/sidebar/App.communication-reconcile.test.tsx` (new)

## Change 1, `src/sidebar/stores/sessions.ts`

Add a generation counter for communication edges, mirroring `waitingEdgeGeneration` exactly.

- Beside `let waitingEdgeGeneration = 0;` at line 25, add `let communicationGeneration = 0;`.
- Beside the getter `get waitingEdgeGeneration()` at lines 392-394, add a
  `get communicationGeneration()` returning it.
- Inside `setCommunication` (lines 557-559), add `communicationGeneration += 1;` before the
  `setState` call.

Do not add a second setter. The poll below reuses `setCommunication` and therefore advances the
counter itself; that is handled by evaluating the guard once, before the loop, which is the pattern
the surrounding code already documents.

## Change 2, `src/sidebar/App.tsx`, inside `refreshProfileOutdated` (lines 316-338)

- After `const generationAtRequest = sessionsStore.waitingEdgeGeneration;` (line 318), capture
  `const communicationGenerationAtRequest = sessionsStore.communicationGeneration;`.
- Beside `const snapshotIsCurrent = ...` (lines 325-326), add
  `const communicationSnapshotIsCurrent = sessionsStore.communicationGeneration === communicationGenerationAtRequest;`.
  It must be evaluated ONCE here, before the loop, for the reason already written in the comment at
  lines 321-324: the writes inside the loop advance the counter themselves.
- Inside `for (const s of list)`, immediately AFTER
  `sessionsStore.setProfileOutdated(s.id, s.profileOutdated ?? false);` (line 328) and BEFORE the
  `continue` on line 329, add:
  `if (communicationSnapshotIsCurrent) sessionsStore.setCommunication(s.id, s.communication ?? null);`

Placement is load-bearing and must not move. The `continue` on line 329 skips every session that is
not working, and a session waiting on a menu is not working. Putting the new line after that
`continue` would reconcile exactly the sessions this epic does not care about.

`?? null` is load-bearing too: `communication` is `skip_serializing_if = "Option::is_none"` on the
Rust side, so a session with no communication arrives with the key ABSENT, not as `null`. Writing
`s.communication` alone would store `undefined` and leave a stale notice on screen forever.

## Required behaviour

- Every five seconds, and on every window focus or visibility change, each session's `communication`
  in the store equals what the backend's listing says.
- A session whose listing omits `communication` has its store entry set to `null`.
- A `session_communication_changed` event that lands while a listing is in flight wins: the whole
  reconcile for that snapshot is skipped and the next tick applies a fresh listing.

## Failure behaviour

- `SessionAPI.list()` throwing is already caught at `src/sidebar/App.tsx:335-337` and logged. The
  reconcile is skipped and the next tick retries. Do not add new error handling.
- The poll is skipped entirely while the window is hidden (`src/sidebar/App.tsx:346-349`). That is
  existing, intended behaviour: it resumes on focus. Do not change it.

## Tests

New file `src/sidebar/App.communication-reconcile.test.tsx`, starting with
`// @vitest-environment jsdom`, built on the existing harness used by
`src/sidebar/App.menu-guard.workflow.test.tsx`. Import `renderWithFakeTransport`, `baseSettings`,
`session`, `waitFor` and `resetUiStoresForTests` from `src/shared/testing/ui-harness`, and import
`FakeTransport` from `src/shared/testing/fake-transport`. `ui-harness.tsx:13` imports `FakeTransport`
from `./fake-transport` and does NOT re-export it, which is why
`App.menu-guard.workflow.test.tsx:4` takes it from the other module. Advance the five-second interval
with vitest fake timers.

1. **Recovers a lost notice.** Session listed with a visible `blockedMenu` communication. Render, let
   the store settle, then overwrite the store with `sessionsStore.setCommunication(id, null)` to
   simulate the loss. Advance the poll. Assert the store entry is the `blockedMenu` object again.
2. **Clears a stale notice.** Store holds a visible `blockedMenu`; the listing returns that session
   with no `communication` key at all (omit it, do not set it to `null`, so the absent-key case is
   what is actually exercised). Advance the poll. Assert the store entry is `null`.
3. **Reconciles a session that is not working.** The session in test 1 must have a status that makes
   `isSessionWorking` false. This is the regression test for the placement rule above: move the new
   line below the `continue` and this test must fail.
4. **An event in flight wins.** Make the fake transport's `list_sessions` resolve on a promise you
   control. Start the poll, then, while it is pending, call `sessionsStore.setCommunication(id, X)`
   with a value different from the listing's. Resolve the listing. Assert the store still holds `X`.
5. **Absence is a typed state.** An empty session list must complete the poll without throwing and
   must not touch any store entry.

## Verification command

Run from the repository root:

```
npm run typecheck
npx vitest run src/sidebar/App.communication-reconcile.test.tsx src/sidebar/App.profile-drift.test.tsx
npm test
npm run check:frontend-dependencies
git status --porcelain
```

## Acceptance criteria

1. `npm run typecheck` exits 0 with no output.
2. The targeted vitest run reports 5 new tests passing and
   `src/sidebar/App.profile-drift.test.tsx` still passing, with 0 failures. That second file is in
   the command because it covers the same function; a change there is a regression, not a rename.
3. `npm test` reports every suite passing. If the process exits 1 while the summary says all tests
   passed, that is the known issue-480 signature and is accepted; any other failure is not.
4. `npm run check:frontend-dependencies` exits 0. This phase adds no module arc, so a cycle report
   here means something unrelated broke and the phase stops.
5. `git status --porcelain` lists exactly the three files named above and nothing else.
6. Deleting `if (communicationSnapshotIsCurrent)` makes test 4 fail; moving the new line below the
   `continue` on line 329 makes test 3 fail; changing `?? null` to bare `s.communication` makes test
   2 fail. Each of the three must be checked once, by hand, and reverted.

## Preserve list

- Do not change the interval at `src/sidebar/App.tsx:781` or the hidden-window guard at `:346-349`.
- Do not change the existing waiting-mirror logic at `src/sidebar/App.tsx:329-333` or its generation
  counter. This phase adds a parallel counter; it does not reuse or widen `waitingEdgeGeneration`.
- Do not touch the event listener at `src/sidebar/App.tsx:729-761`. That is P2's file region.
- Do not touch any Rust file. The backend already sends everything this phase needs.
