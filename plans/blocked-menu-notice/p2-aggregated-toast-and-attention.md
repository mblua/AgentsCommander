# P2: one derived aggregated toast, un-evictable, plus the taskbar flash

Class: design-bearing. Owner: frontend. Depends on: P1.
Status: READY_FOR_IMPLEMENTATION

## Objective

Replace the N per-session sticky toasts with exactly one toast, derived from state rather than pushed
from an event, that cannot be evicted, cannot be stranded by the dismiss animation, and flashes the
taskbar when a session blocks while the window is unfocused. Three closed product decisions shape it
and must not be reinterpreted: the toast stays until the user removes it; it does flash the taskbar;
and its actions target the session whose text is on screen, never the oldest.

## Exact files

1. `src/shared/stores/toasts.ts`
2. `src/shared/stores/toasts.test.ts`
3. `src/sidebar/attention.ts` (new)
4. `src/sidebar/attention.test.ts` (new)
5. `src/sidebar/App.tsx`
6. `src/sidebar/App.menu-guard.workflow.test.tsx`

Implementation order is fixed: file 1, then 3, then 5; the effect needs `pinned` to compile.

## Change 1, `src/shared/stores/toasts.ts`: `pinned`

- `PushToastOptions` (lines 13-23): add `pinned?: boolean;` with a comment saying it exempts the
  toast from kind-aware eviction, that exactly one such toast exists (the blocked-menu aggregate),
  and that `MAX_VISIBLE` therefore stays an honest cap.
- `Toast` (lines 25-33): add `pinned?: boolean;`.
- In `push`, carry `pinned: opts.pinned` into the new toast object (lines 115-123).
- Replace the victim search at lines 131-135 with three ordered tiers:

```
while (next.length > MAX_VISIBLE) {
  let victim = next.findIndex((t) => t.kind !== "error" && !t.pinned);
  if (victim === -1) victim = next.findIndex((t) => !t.pinned);
  if (victim === -1) victim = 0;
  evicted.push(next.splice(victim, 1)[0].id);
}
```

The third tier is unreachable while only one toast is ever pinned, and it is there so the loop always
terminates and the cap never silently becomes 5.

Name the trade-off: with four sticky errors visible and the pinned aggregate arriving, tier 1 finds no
victim and tier 2 evicts an ERROR, which is exactly what the kind-aware rule was written to prevent.
Deliberate: a blocked session is unanswerable work the user asked to be un-losable; an error is a report.

TWO comment blocks now claim the opposite and BOTH must be rewritten to describe the three tiers and
the pinned exemption. Lines 43-46, the policy block above `MAX_VISIBLE`. And lines 127-130, directly
above the victim search this change replaces, which is worse: line 128 asserts the exact guarantee
tier 2 breaks, "an unread sticky error is never silently dropped by a transient toast", and line 129
becomes literally FALSE, because the fallback will also fire when the only unpinned toasts are errors
(tier 2) and again unconditionally at index 0 (tier 3), not only "when all visible toasts are errors".
Acceptance criterion 8 makes this checkable instead of leaving it an intention.

## Change 2, `src/shared/stores/toasts.ts`: the dismiss/re-push race

`startToastExit` (lines 81-94) sets `exiting: true` and schedules removal 180 ms later. The tag branch
of `push` (lines 103-111) finds that dying toast and patches its message without cancelling anything,
so the new message is painted and then deleted at t=180 exactly, counted from the dismiss.

In the tag branch, before the three `setToasts` patches, revive the toast when it is dying:

```
const existing = toasts[existingIndex];
if (existing.exiting) {
  clearExitTimer(existing.id);
  setToasts(existingIndex, "exiting", false);
}
```

`clearExitTimer` is already declared at lines 63-69, above `push`. Also patch `pinned` alongside
`message`, `action` and `secondaryAction` so a revived toast keeps the pin.

Inside that same `if (existing.exiting)` block, and ONLY there, re-arm the auto-dismiss timer from
`opts.durationMs === undefined ? DEFAULT_DURATION_MS[existing.kind] : opts.durationMs` when that value
is not `null`. `startToastExit` cleared the duration timer at line 82, and the tag branch returns at
line 109 without re-arming it, so a revived non-sticky tagged toast would otherwise stay on screen
forever. It is a no-op for the aggregate, whose `durationMs` is `null`.

Be accurate about who else this helps: after change 4 deletes `src/sidebar/App.tsx:740`, NO tagged
caller remains in `src/` outside tests (the surviving `tag:` hits in `automation-bridge.ts` and
`types.ts` are not `PushToastOptions.tag`), so the re-arm's only current consumer is test 20. Keep it:
this phase introduces the revive, and a shared store whose revive silently drops a timer is how the
next caller inherits a bug. Re-arm ONLY on the revive path, never on every tag update: re-arming on
every update would stop a repeatedly re-pushed info toast from ever auto-dismissing, which IS a
behaviour change for existing callers. `DEFAULT_DURATION_MS[existing.kind]` is the right source
because the tag branch never patches `kind`, and it must stay that way: existing callers rely on
`kind` being fixed at creation, and widening this branch further is outside this phase.

## Change 3, `src/sidebar/attention.ts` (new)

```
import { isTauri } from "../shared/platform";

export async function requestTaskbarAttention(): Promise<void> {
  if (!isTauri) return;
  if (typeof document !== "undefined" && document.hasFocus()) return;
  try {
    const { getCurrentWindow, UserAttentionType } = await import("@tauri-apps/api/window");
    await getCurrentWindow().requestUserAttention(UserAttentionType.Critical);
  } catch (err) {
    console.error("[blocked-menu] requestUserAttention failed:", err);
  }
}
```

This mirrors `src/main/components/ErrorModal.tsx:84-93`, guard and dynamic import included. It lives in
`src/sidebar/` because that layer already owns the window surface; the same call inside
`src/shared/stores/toasts.ts` would give a store a UI-transport dependency and is forbidden.

## Change 4, `src/sidebar/App.tsx`: the listener loses the toast

In the `onSessionCommunicationChanged` handler (lines 729-761), keep line 730,
`sessionsStore.setCommunication(sessionId, communication);`, and delete the entire `if`/`else` that
follows it, **lines 731-760**. The `else` closes on line 760, not 759; deleting 731-759 leaves an
orphan brace. The per-session tag `blockedMenu:${sessionId}` disappears from the codebase; the
aggregate uses one fixed tag.

Line 734's guard, `communication.message`, disappears with that block, and change 5's memo must
re-create it as a filter condition: `message` is `string | null | undefined` on the wire
(`src/shared/types.ts:18`), and without the guard the template renders the literal `null`.

## Change 5, `src/sidebar/App.tsx`: the derived effect

Add `export const BLOCKED_MENU_TAG = "blockedMenu";` at MODULE level in `src/sidebar/App.tsx`,
exported so the test file can import it for test 19's filter. Inside the COMPONENT scope, not module
level, add a plain non-reactive `const flashedBlockedSessions = new Set<string>();`: it is per-instance
state, and a module-level `Set` would be shared by two `App` instances mounted at once, so the second
would never flash. Then:

- a `createMemo` returning a plain array of VALUE OBJECTS, one per session whose `communication` is a
  visible `blockedMenu` with a non-empty `message`, each object carrying `id`, `name`, `message` and
  `updatedAt`, sorted newest first by `Date.parse(updatedAt)`.

  Reading `message` and `updatedAt` inside that `map` is load-bearing, and is the whole reason the memo
  returns value objects instead of session rows. A memo that filters only on `kind` and `visible` and
  reads `updatedAt` in a `sort` comparator is not subscribed to `message` at all, and with one element
  `Array.prototype.sort` never invokes the comparator, so not to `updatedAt` either. The toast would
  then keep the previous text forever.

  That is a supported transition, not a corner case. `set_blocked_menu`
  (`src-tauri/src/session/manager.rs:1019-1035`) returns `changed = false` only when the incoming
  message is byte-identical; otherwise it overwrites in place with `visible: true`, never passing
  through a non-visible state. So a session moving from one blocking menu to another keeps `kind` and
  `visible` and changes only `message` and `updated_at`, and the repo already ships two such menus for
  codex (`src-tauri/src/config/settings.rs:1035` and `:1013`). Today the listener re-pushes on every
  event and the tag branch patches the text; change 4 deletes that, so the memo becomes the only thing
  left that can notice.

  Parse `updatedAt`; do not compare the strings. It is RFC3339 from
  `chrono::Utc::now().to_rfc3339()`, and a lexicographic comparison is only accidentally correct while
  the offset and the fractional digits never vary.
- a `createEffect` that reads that memo and then, INSIDE `untrack(...)` from `solid-js`:
  - when the list is empty: `toastStore.dismissByTag(BLOCKED_MENU_TAG)` and clear
    `flashedBlockedSessions`;
  - otherwise: push one toast with `tag: BLOCKED_MENU_TAG`, `kind: "info"`, `durationMs: null`,
    `pinned: true`, whose message is `` `${newest.name}: ${newest.message}` `` when the list has one
    entry, and that same text followed by `` ` (and ${n - 1} more waiting)` `` otherwise. Build the
    text from the memo's value objects only. Do not reach back into `sessionsStore` here. Measured:
    the forbidden variant still repaints, because the subscription lives in the MEMO and not in the
    effect body, so this is hygiene, exactly like `untrack`, and criterion 6 is right not to list it
    as a falsifier. Keep the rule anyway: it keeps the memo the single source and stops a later
    refactor from carrying the `message` filter away with it.
    `secondaryAction` is `{ label: "See terminal", dismissOnClick: false, onClick: () => void SessionAPI.switch(newest.id).catch(() => {}) }`
    and `action` is `{ label: "Resolved by user", onClick: () => void resolveBlockingMenu(newest.id) }`,
    both taking `newest.id`, never the oldest;
  - then, for every id in the list not already in `flashedBlockedSessions`, add it and, if at least
    one was added, call `void requestTaskbarAttention()`; finally drop from the set every id no
    longer in the list.

`untrack` is hygiene, not a hang preventer, and the distinction matters because test 19 and acceptance
criterion 6 are built on it. Measured on `solid-js@1.9.12`, the version `package-lock.json` pins
(`package.json:46` declares the range `^1.9.12`), against a faithful copy of
`src/shared/stores/toasts.ts` and this effect: one arriving blocked session runs the effect ONCE with
`untrack` and TWICE without it, and the toast array settles at 1 either way. It does not loop.
`toastStore.push` subscribes the effect only to `toasts.length`, to each item's `tag`
(`src/shared/stores/toasts.ts:104`) and to that item's `id` (`:109`); the `toasts.some` at `:141` is
never evaluated because `duration !== null &&` short-circuits when `durationMs` is `null`; and the tag
branch writes `message`, `action` and `secondaryAction` (`:106-108`), none of which the effect reads.
The single extra run is `length` going 0 to 1, and it stops there.

Keep `untrack` anyway: it saves that run, states the intent, and keeps the effect's dependency set
equal to the memo. But do not claim it prevents a hang, and never write a test whose stated failure
mode is a hang: `src/` contains no `ErrorBoundary`, and Solid's only bound, `Updates.length > 10e5`,
sits on the PURE computation queue that a `createEffect` never enters, so a genuine runaway would
wedge the vitest worker instead of failing it. Test 19 asserts an exact run count instead.

`flashedBlockedSessions` is a plain `Set`, deliberately not a signal. Writing to a signal the effect
reads is a genuine self-trigger, and there is no reason to introduce one here.

## Required behaviour

- Zero blocked sessions: no aggregated toast, no flash.
- One blocked session: one toast carrying that session's own notice text verbatim; both buttons act
  on that session.
- N blocked sessions: still exactly one toast, showing the newest session's verbatim text plus
  `(and N-1 more waiting)`. Resolving the one on screen makes the toast repaint with the next one,
  because the set changed. Resolving the last one dismisses it.
- The SAME session moving from one blocking menu to another, with the set unchanged and only
  `message` and `updatedAt` changing, repaints the toast with the new text, dismissed or not, because
  that is information the user has not seen. The current listener already does this; do not lose it.
- The toast survives four simultaneous sticky errors. That is the measured case that kills it today.
- The user dismissing the toast keeps it dismissed while the set does not change, and a newly blocked
  session raises it again. Measured: after a dismiss, the P1 poll rewriting an IDENTICAL communication
  does NOT bring it back, because Solid's store merges key by key and notifies nothing when the
  content is unchanged, so the effect does not even run. Making a dismiss also silence that session's
  later menus would be a product change; nobody asked for it, so do not build it.
- The taskbar flashes once per session entering the blocked set, only while the window is unfocused,
  and never again for that session until it leaves the set and re-enters.

## Failure behaviour

- `requestTaskbarAttention` never throws into the effect: it catches and logs. A missing or failing
  Tauri window API degrades to no flash; the toast and the row indicator are unaffected.
- `SessionAPI.switch` already swallows its own rejection; keep the `.catch(() => {})`.
- `resolveBlockingMenu` failing leaves the backend untouched, the set unchanged and the toast up. That
  is correct: the menu really is still unanswered.

## Why there is no runtime self-healing loop

An earlier draft proposed that the effect also watch the toast store and re-push when the aggregate
disappears, and justified banning it with an infinite loop. There is no infinite loop; see change 5.
The two real reasons stand: reading `toasts` reactively would re-run this effect on every one of the
eighteen unrelated toast sources listed under test 19, so the run count test 19 discriminates on would
stop meaning anything; and it would resurrect a toast the user deliberately dismissed, contradicting a
closed decision. Make the loss impossible instead: changes 1 and 2 close both measured loss paths and
criterion 6 kills each revert on an assertion. The always-present record is the sidebar row and, after
P4, its rollups.

## Tests

### `src/shared/stores/toasts.test.ts` (extend)

1. **The measured threshold.** Push four sticky errors, then one `pinned` info toast. Assert it is
   alive. Without `pinned` this is the case that returns a valid id for a toast that never entered
   the array, so assert on the array, never on the returned id.
2. **The threshold is exactly four.** Repeat test 1 at zero, one, two and three prior errors and
   assert the pinned toast is alive in every case. This pins the measurement: with 0 to 3 it survives
   even today, and only the fourth error kills it.
3. **A pinned toast is never the victim.** Four errors plus one pinned info, then push a fifth error.
   Assert the pinned toast is still alive and that an error was evicted instead.
4. **The cap is still honest.** With one pinned toast up, push four more non-pinned toasts and assert
   the array length is exactly `MAX_VISIBLE`, four.
5. **The physical-ceiling contract.** Push five plain toasts, assert exactly four survive, and carry a
   comment in the test stating that `.toast-host` in `src/shared/styles/toast.css` declares no
   `overflow` and no `max-height`, so this cap is the only thing bounding the stack height. This
   replaces host scrolling, which is out of scope for this epic.
6. **The revive.** Use fake timers. Push a tagged toast, `dismissByTag`, advance 50 ms, push the same
   tag with a new message, advance past 180 ms from the original dismiss, and assert the toast is
   still alive with the new message. Reverting change 2 must make this fail at exactly 180 ms.
7. **A normal dismiss still removes.** Push a tagged toast, dismiss it, advance past 180 ms with no
   re-push, and assert it is gone. This is the control for test 6.
20. **The revive re-arms auto-dismiss.** Fake timers. Push a tagged toast with an explicit finite
    `durationMs`, `dismissByTag` it, advance 50 ms, re-push the same tag, then advance past that
    duration with no further interaction. Assert the toast is gone. Without the re-arm it stays up
    forever, because `startToastExit` cleared its duration timer at line 82 and the tag branch
    returns at line 109. Pair it with an assertion that a `durationMs: null` toast revived the same
    way is still alive after the same wait, so the no-op case is pinned too.

### `src/sidebar/attention.test.ts` (new)

This file MUST start with `// @vitest-environment jsdom`. `vitest.config.ts:12` sets
`environment: 'node'` for the whole suite, and tests 8 and 9 call
`vi.spyOn(document, "hasFocus")`, which has no `document` to spy on under the node environment.

`vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false, isWindows: true }))` and
`vi.mock("@tauri-apps/api/window", ...)` returning a `getCurrentWindow` whose `requestUserAttention`
is a `vi.fn()`, plus a `UserAttentionType` object.

8. Unfocused window (`vi.spyOn(document, "hasFocus").mockReturnValue(false)`): the mock is called once
   with `UserAttentionType.Critical`.
9. Focused window: the mock is not called at all.
10. `isTauri` false, via a separate mock: the mock is not called and nothing throws.
11. The dynamic import rejecting: `requestTaskbarAttention()` resolves and does not throw.

### `src/sidebar/App.menu-guard.workflow.test.tsx` (extend)

`vi.mock("./attention", () => ({ requestTaskbarAttention: vi.fn() }))`.

12. **One blocked session.** The single toast carries that session's verbatim notice text and its
    name, and no "more waiting" suffix.
13. **Ten blocked sessions.** Exactly one toast exists. Its text is the NEWEST session's notice by
    `updatedAt`, and it ends with `(and 9 more waiting)`. Give the ten different `updatedAt` values
    and different `message` values, and deliberately make the newest not be the last in array order,
    so a test that passes by accident of ordering cannot.
14. **Resolve the one on screen.** From the ten, clear the newest in the store. Assert the toast now
    shows the second-newest, and still one toast.
15. **Empty set dismisses.** Clear the last one; assert the aggregate toast is gone.
16. **The flash fires once.** One session becomes blocked with the window unfocused: the mock is
    called once. Re-apply the identical communication (the P1 poll doing its job): still once.
17. **Re-arm.** Clear that session, then block it again: the mock has now been called twice.
18. **No flash for an empty set.** Never blocked, poll runs: the mock is not called.
19. **Exact effect-run count.** `const pushSpy = vi.spyOn(toastStore, "push")`. Render with exactly
    one blocked session and let it settle. Assert:

    ```
    expect(pushSpy.mock.calls.filter((c) => c[0]?.tag === BLOCKED_MENU_TAG)).toHaveLength(1);
    ```

    Filtering by the tag is mandatory, not stylistic. `vi.spyOn` replaces the `push` property on the
    shared `toastStore`, and `toastStore.error`, `.info` and `.success`
    (`src/shared/stores/toasts.ts:168`, `:171`, `:174`) call it back THROUGH that property, so an
    unfiltered counter counts the whole sidebar. EIGHTEEN sites reachable from a mounted `App`
    increment it, all under `src/sidebar/`: `App.tsx:292`, `:499`; `stores/project.ts:455`;
    `update-toast.ts:21`; `agent-update.ts:138`, `:368`, `:398`; `listeners-screenshot.ts:30`, `:38`,
    `:46`; `components/AgentUpdateOverlay.tsx:139`, `:143`, `:152`;
    `components/SettingsModal.tsx:1568`, `:1575`; `components/ProjectPanel.tsx:518`, `:2192`, `:3262`.
    That is the whole census: nineteen non-test `toastStore.(error|info|success|push)(` matches UNDER
    `src/sidebar/`, minus `App.tsx:736` which change 4 deletes. The directory scope is load-bearing:
    the same grep over all of `src/` returns 22, catching the three wrappers in `toasts.ts` itself.
    The overlay is mounted unconditionally (`App.tsx:83`, `:961`), and `App.tsx:499` is a
    session-warning error toast, so a workflow test mounting the sidebar with sessions is exactly
    where an unfiltered counter moves. Whether today's mount is quiet is beside the point: an
    unfiltered assertion is a flake waiting for one of those to fire, and the natural repair for a
    flake is to loosen it, destroying the 1-versus-2 discrimination this test exists for.

    Every non-empty run of the effect calls `push` exactly once with that tag, so this counts effect
    runs without adding any test-only surface to `src/sidebar/App.tsx`. The measured value without
    `untrack` is 2, so the revert dies on a number, in milliseconds, every time. Do NOT assert "the
    array length stays 1": Solid's reactive cycle is synchronous, so flushing microtasks cannot
    observe re-entry and that assertion is decorative.
21. **Same set, different message, the toast repaints.** One blocked session. Let it settle, then
    write a new `communication` for that same session with a DIFFERENT `message` and a later
    `updatedAt`, keeping `kind: "blockedMenu"` and `visible: true`. Assert the visible toast text now
    contains the new message and not the old one, and that there is still exactly one toast. This is
    the regression test for the memo reading `message` in tracked scope; a memo that filters only on
    `kind` and `visible` leaves the old text on screen and this test fails.

## Not automatable, and who verifies it

Everything above tests that the app ASKS for attention. Nothing can test that Windows actually flashes
the taskbar button: `requestUserAttention` returns void, no API reads the flag back, and the only ways
to observe it are a screen capture or an input device, all excluded. Tests 8 to 11 (the call) and 16
to 18 (when) are the whole automatable surface. The rest is a manual check by the user, four steps:

1. Start the app, open a session with a coding agent that has a blocking menu configured, and let it
   reach an interactive menu.
2. Move focus to another application, so the AgentsCommander window is not focused.
3. Confirm the taskbar button flashes and stops when the window is focused.
4. Click into the window, resolve the menu, block it again, and confirm the flash repeats.

State this in the PR body. Do not claim the flash is covered by tests.

## Verification command

```
npm run typecheck
npx vitest run src/shared/stores/toasts.test.ts src/sidebar/attention.test.ts src/sidebar/App.menu-guard.workflow.test.tsx
npm test
npm run check:frontend-dependencies
git status --porcelain
```

## Acceptance criteria

1. `npm run typecheck` exits 0 with no output.
2. The targeted run reports 0 failures, and every one of the 21 numbered tests above is present and
   passing. The reported total will be HIGHER than 21: that command also runs the tests those three
   files already contain. Check the names, not the total.
3. `npm test` reports every suite passing. Exit 1 with an all-passed summary is the known issue-480
   signature and is accepted; nothing else is.
4. `npm run check:frontend-dependencies` exits 0. This phase adds one module arc,
   `src/sidebar/App.tsx` to `src/sidebar/attention.ts`, whose target imports only
   `src/shared/platform.ts` (which imports nothing) and a dynamic external module. A cycle report
   there means the new file grew an import it must not have; stop and cut it.
5. `git status --porcelain` lists exactly the six files named above and nothing else.
6. Six reverts, each checked once by hand and undone, each dying on an assertion rather than on a
   hang: removing `pinned` from the victim search makes test 1 fail; removing the revive block makes
   test 6 fail at t=180; removing the duration re-arm makes test 20 fail; removing `untrack` makes
   test 19 report 2 instead of 1; dropping `message` and `updatedAt` from the memo's `map`, so it
   filters only on `kind` and `visible`, makes test 21 fail; targeting the oldest instead of
   `newest.id` in either action makes test 13 or 14 fail.

   Test 2 is deliberately NOT in that list. It passes against the broken implementation in all four
   of its sub-cases, by design: it is a characterisation test that pins where the measured threshold
   is, not a falsifier. Test 1 is the falsifier for that behaviour.
7. A grep for `blockedMenu:` across `src/` returns nothing outside test fixtures. The per-session tag
   is gone.
8. Both stale comment blocks in `src/shared/stores/toasts.ts` were rewritten, so that change 1's
   instruction is a requirement and not an intention: `grep -n "never silently"
   src/shared/stores/toasts.ts` must return nothing, and both the block above `MAX_VISIBLE` and the
   block above the victim search must contain the word `pinned`. Criteria 1 to 7 passing while this
   one fails is a failed phase: the code would ship claiming a guarantee it no longer makes, which is
   the exact defect this epic exists to stop happening to a notice.

## Preserve list

- Do not remove or weaken `MAX_VISIBLE = 4`. It is the only bound on the stack height.
- Do not change kind-aware eviction for unpinned toasts. Errors must still outrank info toasts.
- Do not add `overflow` or `max-height` to `.toast-host` in `src/shared/styles/toast.css`. Out of
  scope, and `pointer-events: none` on line 13 would make that scrollbar unusable anyway.
- Do not touch `src/shared/components/ToastHost.tsx`. The rendering is unchanged.
- Do not touch `src/sidebar/App.tsx:730`, the `setCommunication` call. The listener keeps that line.
- Do not touch any Rust file, and do not change `resolve_blocking_menu` or its IPC wrapper.
- Do not build a list view of the N pending notices. The user closed that: one at a time is enough.
