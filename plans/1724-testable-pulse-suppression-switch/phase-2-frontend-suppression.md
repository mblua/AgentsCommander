# Phase 2 (#1724): Frontend suppression and its observable trace

Status: READY_FOR_IMPLEMENTATION
Class: `design-bearing`
Owner: `ac-dev-webpage-ui-v4`
Depends on: Phase 1 (`phase-1-rust-gate-and-command`), which must be landed on the branch first because this phase invokes the Tauri command it registers. Parallel with: Phase 3 (`phase-3-docs`), which shares no file.
Branch: `feature/1724-testable-pulse-suppression-switch`, base `1e57aa581de4c4fd18590cdf0652d8bf60b18a4f`.
Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-AgentsCommander`.

## Objective

Read the suppression flag once at main-window mount, and when it is on, complete every layout pulse as `status: "skipped"`, `reason: "suppressed"` before any sidebar width is written, so a harness can prove through the `ui-terminal` query that the pulse was actually off rather than assume it.

## Exact files (freeze this set; nothing else may change)

1. `src/shared/types.ts`
2. `src/shared/ipc.ts`
3. `src/main/App.tsx`
4. `src/main/App.pulse-suppression.test.tsx` (new file)

## Decisions, all already made

**Where the flag comes from.** Phase 1's Tauri command `ui_automation_layout_pulse_suppressed`, which returns `true` only when the process runs `agentscommander_testeable.exe`, has UI automation enabled, and was started with `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE=1`. The frontend never re-derives the gate and never reads any environment itself.

**When it is read.** Exactly once, as the first statement of `MainApp`'s `onMount`, inside its own `try`/`catch`, and gated on `isTauri`. Not on every request, not lazily, and never again afterwards.

**Why that placement is race-free, and this is the load-bearing argument of the phase.** `runPulse` has exactly two call sites: one inside `onMainTerminalLayoutPulseRequest` guarded by `if (sidebarInitializationSettled)`, and one in the `finally` of `onMount` immediately after `sidebarInitializationSettled = true`. Resolving the flag as the first statement of `onMount`, in a `try`/`catch` that precedes the outer `try` the `finally` belongs to, means the flag is final on every path, including the path where the outer `try` throws, before either call site can execute. A request arriving before resolution is parked by the pre-existing mechanism and evaluated afterwards. Do not move the read into the outer `try`, into the request handler, or into a lazily awaited promise: any of those reintroduces the race this design exists to eliminate.

**Where the check sits.** At the top of `runPulse`, after its existing first guard and before `owner.started = true`. That is roughly sixty lines before the first `setSidebarWidth(nudgedWidth)`, so no width is ever written on the suppressed path. Do not place it in `onMainTerminalLayoutPulseRequest`: that would run before the flag is guaranteed resolved.

**Reason precedence, accepted deliberately.** The pre-existing `busy`, `dragging` and `persistence_owned` rejections happen in the request handler and therefore still win over suppression. That is correct: in each of those cases the pulse also did not run and no width was written, and the harness sees the true reason instead of a synthesized one. `"suppressed"` is emitted from exactly one call site, so its presence is unambiguous proof that the switch did it.

**No version bump.** `MainTerminalLayoutPulseTrace.version` stays `1`. The version pins the trace's field set, which is unchanged; `"suppressed"` is a new value of an existing string field. Rust carries the trace as an opaque `serde_json::Value` and models no reason vocabulary, so nothing on the Rust side moves.

**No runtime toggle.** The value is process-lifetime and immutable, so no in-flight pulse can observe it change. There is nothing to cancel and no mid-flight state to define.

## Edit 1: `src/shared/types.ts`

`MainTerminalLayoutPulseReason` is at lines 1101-1116. Insert one member immediately after the `| "clamped"` line, so the two skips that `runPulse` itself decides sit together:

```ts
  | "suppressed"
```

The union has no exhaustive `switch`, no `never` assertion and no runtime enumeration anywhere in the repository (all eight references to the type name are type positions), so this is purely additive. Change nothing else in this file; in particular `MainTerminalLayoutPulseTrace.version` stays the literal `1`.

## Edit 2: `src/shared/ipc.ts`

`AutomationAPI` is at lines 591-600. Add one method immediately after `enabled`:

```ts
  layoutPulseSuppressed: () =>
    transport.invoke<boolean>("ui_automation_layout_pulse_suppressed"),
```

The command name string must match Phase 1's `#[tauri::command] pub fn ui_automation_layout_pulse_suppressed` exactly. Change nothing else in this file.

## Edit 3: `src/main/App.tsx`

### 3a. Import

Line 16 currently reads:

```ts
import { SettingsAPI } from "../shared/ipc";
```

Replace with:

```ts
import { AutomationAPI, SettingsAPI } from "../shared/ipc";
```

`isTauri` is already imported on line 17; add no other import.

### 3b. The module-local flag

Directly after `let pulseOwner: SidebarPulseOwner | null = null;` (line 184), add:

```ts
  // #1724 - resolved once, as the first statement of onMount, and never written again.
  // Both `runPulse` call sites are gated on `sidebarInitializationSettled`, which that
  // same onMount sets in its `finally`, so this value is always final before a pulse can
  // start. That is why there is no runtime toggle and no in-flight race to handle.
  let layoutPulseSuppressed = false;
```

### 3c. The guard in `runPulse`

`runPulse` opens at line 486:

```ts
  const runPulse = async (owner: SidebarPulseOwner): Promise<void> => {
    if (owner.completed || pulseOwner !== owner || disposed) {
      return;
    }
    owner.started = true;
```

Insert the new block between the closing brace of that guard and `owner.started = true;`:

```ts
    if (layoutPulseSuppressed) {
      finishPulse(owner, "skipped", "suppressed");
      return;
    }
```

`owner.started` deliberately stays `false`; `finishPulse` clears the request watchdog on its own, so the `initialization_timeout` / `request_timeout` distinction is never reached.

### 3d. The resolution, first statement of `onMount`

`onMount` opens at line 883:

```ts
  onMount(async () => {
    try {
      cleanupZoom = await initZoom("main");
```

Replace that opening with:

```ts
  onMount(async () => {
    // #1724 - FIRST, and in its own try/catch, so the flag is final on every path,
    // including the one where the block below throws, before the `finally` sets
    // `sidebarInitializationSettled` and releases a parked pulse.
    try {
      layoutPulseSuppressed = isTauri
        ? await AutomationAPI.layoutPulseSuppressed()
        : false;
    } catch {
      layoutPulseSuppressed = false;
    }

    try {
      cleanupZoom = await initZoom("main");
```

Everything from `cleanupZoom = await initZoom("main");` to the end of `onMount`, including the `finally` block, is unchanged.

## Edit 4: new file `src/main/App.pulse-suppression.test.tsx`

A separate file is mandatory. `src/main/App.sidebar-width.test.tsx` mocks `../shared/platform` to `isTauri: false` and replaces the whole `../shared/ipc` module with a two-method `SettingsAPI` stub; those mocks are per-file and cannot express the switch-on case. Keeping them apart is also what makes issue criterion 1 literally true: the existing pulse tests pass untouched.

Build the new file on the same harness as `App.sidebar-width.test.tsx`, copying its `installManualFrames`, `flushPromises`, `settings`, `dispatchPulse`, `renderMain` and `sidebarWidth` helpers and its `installBrowserDomStubs` usage from `../shared/testing/ui-harness`. Differences from that file, and these are the whole point:

- `vi.mock("../shared/platform", () => ({ isTauri: true }))`.
- The `../shared/ipc` mock exposes both `SettingsAPI` (`get`, `update`) and `AutomationAPI` with a single hoisted `layoutPulseSuppressed` mock.
- `vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, setAlwaysOnTop: async () => {} }) }))`. This is required because `isTauri` is now true and `onMount` reaches `getCurrentWindow().onCloseRequested`. Follow the precedent in `src/terminal/App.workflow.test.tsx:35-37`.
- The `settings()` fixture keeps `mainAlwaysOnTop: false`, so the `setAlwaysOnTop` branch is never taken, and `mainSidebarWidth: 440`, so the pane starts at `"440px"`.
- `@tauri-apps/api/webviewWindow` does **not** need mocking: it is only imported from inside `countDetachedWindows`, which nothing in these tests calls.

Four tests, all required:

1. **`suppresses the pulse and reports reason "suppressed"`.** `layoutPulseSuppressed` resolves `true`. Render, `await flushPromises()`, then `dispatchPulse`. Assert, without advancing a single frame: `request.accepted === true`; `complete` was called exactly once; its argument is `{status: "skipped", reason: "suppressed", trace: {...}}` where the trace has `version: 1`, the dispatched `requestId` / `sessionId` / `attachGeneration`, `status: "skipped"`, `reason: "suppressed"`, `dwellMs: 0`, `settingsWritesDelta: 0`, and `original`, `expanded` and `restored` each with every field `null`; `sidebarWidth(root)` is still `"440px"`; `dependencies.settingsUpdate` was never called. Asserting the empty phase traces matters: it proves `runPulse` returned before it even sampled a width, which is stronger than observing that the divider did not move.
2. **`a request parked before initialization settles is still suppressed`.** `layoutPulseSuppressed` resolves `true`. Render and `dispatchPulse` **without** flushing first. Assert `request.accepted === true` and `complete` not yet called. Then `await flushPromises()` and assert `complete` was called with `reason: "suppressed"` and that `sidebarWidth(root)` was `"440px"` at every observation. This is the regression test for the race-freedom claim; if the flag were resolved anywhere later than the first statement of `onMount`, this test goes red.
3. **`runs the pulse normally when the switch is off`.** `layoutPulseSuppressed` resolves `false`. Render, `await flushPromises()`, `dispatchPulse` with a live sample. Assert `sidebarWidth(root)` is `"424px"` (440 minus the 16 px inward nudge) and `complete` has not been called. This proves the off path is intact under `isTauri: true`.
4. **`treats a failing bridge as off`.** `layoutPulseSuppressed` rejects. Same assertions as test 3. This proves the `catch` leaves the flag `false` and that a missing or broken command can never silently suppress the pulse in production.

Use `installManualFrames` and never real timers; do not add `vi.useFakeTimers()` beyond what the copied harness does. The dwell is measured from `requestAnimationFrame` timestamps, not from `Date.now()` (`src/main/App.tsx:469-473`), and `installManualFrames` already drives those frames; the only real timer in the pulse is the per-request watchdog, a `setTimeout` armed in `createPulseOwner` (`App.tsx:311-317`) that `finishPulse` clears before it can fire (`App.tsx:213-216`). None of the four tests needs a fake clock, and installing one would only change which of those two mechanisms the test drives.

## Required behavior, edge cases and failure behavior

- Command returns `false`, command throws, transport unavailable, or the build is not Tauri: `layoutPulseSuppressed` stays `false` and the pulse behaves exactly as today.
- Suppression on, request arrives while `dragging()` is true, while a splitter save is pending, or while another pulse owner is live: the pre-existing `dragging` / `persistence_owned` / `busy` reason is reported and `runPulse` is never reached. No width is written in any of those cases either.
- Suppression on, request arrives with a non-integer `requestId` or `attachGeneration`, or a non-string `sessionId`: the pre-existing `failPulseForInvalidNumbers` path still wins, because it runs in the request handler before parking.
- Suppression on, component torn down between acceptance and `runPulse`: `runPulse`'s existing first guard (`disposed`) returns before the suppression check, and `onCleanup` finishes the owner as `cancelled` / `teardown`, unchanged.
- Suppression on and `snapshot.data.length === 0`: no pulse is requested at all by `TerminalView`, so there is no trace and `ui-terminal` reports no `layoutPulse`. That is pre-existing behavior and is out of scope.

## Verification commands, from the repository root

```
npm run typecheck
npx vitest run src/main/App.pulse-suppression.test.tsx
npx vitest run src/main/App.sidebar-width.test.tsx
npx vitest run src/terminal/components/TerminalView.attachment.test.tsx src/shared/automation-bridge.test.ts
npm test
npm run build
npm run check:frontend-dependencies
```

Expected results:

- `npm run typecheck`: exit 0.
- The new file: 4 tests, all passing. `console.log` is intercepted by vitest by default; pass `--disable-console-intercept` if you need to see output while debugging.
- `src/main/App.sidebar-width.test.tsx`: passes with the file byte-identical to the base SHA.
- The attachment and automation-bridge suites: unchanged pass.
- `npm test`: no new failure relative to a base-SHA run. Classify any failure against that baseline before attributing it to this phase.
- `npm run build`: exit 0.
- `npm run check:frontend-dependencies`: pass. This gate is not wired into CI, so it must be run locally; the phase adds no import edge that does not already exist (`src/main/App.tsx` already imports from `../shared/ipc` and `../shared/types`).

## Acceptance criteria

1. `git status --porcelain` lists exactly the four files above and nothing else.
2. `git diff --stat` does **not** list `src/main/App.sidebar-width.test.tsx`. If that file needed an edit, the design was implemented wrong; stop and escalate rather than editing it.
3. All seven verification commands meet their expected results, with captured output and exit codes.
4. `grep -c '"suppressed"' src/main/App.tsx` is 1 and `grep -c '"suppressed"' src/shared/types.ts` is 1: the reason is emitted from exactly one production call site and declared once.
5. In `src/main/App.tsx`, the line containing `if (layoutPulseSuppressed)` appears **before** the first line containing `setSidebarWidth(nudgedWidth)`, and the line containing `layoutPulseSuppressed = isTauri` appears **before** the line containing `sidebarInitializationSettled = true`. Verify both orderings with `grep -n` and record the line numbers.
6. `MainTerminalLayoutPulseTrace` in `src/shared/types.ts` still declares `version: 1` and its field set is unchanged.
7. No new module arc. `npm run check:frontend-dependencies` passes and no import statement was added other than widening the existing `../shared/ipc` import to include `AutomationAPI`.

## Preserve list (must not change in this phase)

- `src/main/App.sidebar-width.test.tsx`, byte for byte.
- `src/terminal/components/TerminalView.tsx` and every test under `src/terminal/`.
- `src/shared/automation-bridge.ts` and `src/shared/automation-bridge.test.ts`.
- `MainTerminalLayoutPulseTrace.version`, the trace field set, `MainTerminalLayoutPulseStatus`, and the fifteen pre-existing reason members.
- The five pre-existing skip paths (`clamped`, `busy`, `dragging`, `persistence_owned`, and the no-request case) and their reasons.
- Everything under `src-tauri/`, everything under `docs/`, `package.json`, `package-lock.json`.
