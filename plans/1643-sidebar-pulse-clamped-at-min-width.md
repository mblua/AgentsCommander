# Plan #1643: Sidebar layout pulse is clamped dead at the minimum sidebar width

Author: ac-architect-v4, room-4, 2026-08-30 UTC. Full `code-implementation-workflow` path, Round 1 candidate. Owned for implementation and the Step 5 verdict by `ac-dev-webpage-ui-v4`; adversarially reviewed at Step 6 by `ac-dev-rust-grinch-v4`.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1643](https://github.com/mblua/AgentsCommander/issues/1643) — "Sidebar layout pulse is clamped dead at the minimum sidebar width (terminal repaint artifact on every attach while the sidebar is narrow)".

Delivery path: Full.

PARTITION: 1 phase (partition trigger does not apply; see `## 14. Partition outcome`).

## 1. Objective

The #1532 authoritative terminal layout pulse (`runPulse` in `src/main/App.tsx`) must keep working when the main sidebar is narrower than `MAIN_SIDEBAR_MIN_WIDTH + SIDEBAR_PULSE_DELTA_PX` (i.e. `[400, 415]` px). Today the pulse computes a 16 px inward nudge through `clampMainSidebarWidth`, the clamp floors the candidate back to `400`, the exact-equality check fails, and the pulse returns `skipped/clamped` **without ever moving the divider**. The PTY repaint workaround is then dead on every attach while the sidebar stays narrow, and the user sees the repaint artifact on every session attach.

Objective: choose a nudge direction that fits the available headroom. If the sidebar can shrink by the delta, keep today's behavior byte-for-byte. If it cannot shrink but can grow by the delta, nudge outward instead. Only when neither direction fits, report `skipped/clamped` as today. The pulse must still complete a full two-fit acknowledged leg, dwell, and restore cycle in either direction, with the direction-dependent assertions inverted exactly.

## 2. Frozen authority and entry gate

- Repo: `D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-AgentsCommander`.
- Branch: `fix/1643-sidebar-pulse-clamped-at-min-width`, created from `main` at `576094ac79cc50e80cfe309875defccbd38c7b9c` (`576094a`).
- Authoring-time verification (2026-08-30 UTC): committed `HEAD` == `576094ac79cc50e80cfe309875defccbd38c7b9c`, `git status --porcelain` empty.
- Frozen blobs at that SHA (all line numbers below were read from these blobs):
  - `src/main/App.tsx` — blob `0fc411898989e266a7b0e078955597aa5794c3b4`.
  - `src/shared/sidebar-layout.ts` — blob `be18bb4c3e840535255acc541264866ca033a22f`.
  - `src/terminal/components/TerminalView.tsx` — blob `2eff54082311d2b41768fb2b6f1f4d0fbd3dd5b6` (read-only context; not modified).
  - `src/main/App.sidebar-width.test.tsx` — blob `8189fd35123067e04597c0fe7591b5b3820edd94`.
  - `src/shared/types.ts` — blob `30678585bd645942851ee613d408c4d731269671` (read-only; not modified).
- Branch-name validation: `scripts/validate-branch-name.mjs` pattern `^(bug|chore|ci|docs|feat|feature|fix|refactor|style|test)\/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$`, `MAX_SLUG = 50`. `fix/1643-sidebar-pulse-clamped-at-min-width` matches (type `fix`, number `1643`, slug `sidebar-pulse-clamped-at-min-width`, 34 chars). The `validate-branch-name` check will pass.
- `plans/` is gitignored (`.gitignore` line 11). The implementer does not add this plan file to the PR unless the tech lead explicitly asks; if asked, force-add exactly this file with `git add -f plans/1643-sidebar-pulse-clamped-at-min-width.md` and never weaken the ignore rule.
- Certification note: this plan is certified READY at the exact byte content of this file. Any byte change after certification invalidates it and requires a new certification round. The Step 7 certification pass re-runs the authority ritual: fetch `origin/main`; stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA `576094a`. If a quoted line number no longer matches the quoted text, re-anchor on the quoted text, never on the number.

## 3. Cause, with evidence

#1532 introduced `runPulse` (`src/main/App.tsx:484-663`) as the attach-time workaround for PTY repaint artifacts: it narrows the sidebar by `SIDEBAR_PULSE_DELTA_PX = 16` (`App.tsx:43`), waits for the terminal to reflow and acknowledge with a fresh two-fit observer ack, dwells `SIDEBAR_PULSE_DWELL_MS = 200`, then restores the original width. The nudge is computed through the shared clamp (`App.tsx:510-518`):

```ts
      const nudgedWidth = clampMainSidebarWidth(
        originalWidth - SIDEBAR_PULSE_DELTA_PX,
        window.innerWidth,
      );
      if (nudgedWidth !== originalWidth - SIDEBAR_PULSE_DELTA_PX) {
        finishPulse(owner, "skipped", "clamped");
        return;
      }
```

`clampMainSidebarWidth` (`src/shared/sidebar-layout.ts:9-13`) floors at `MAIN_SIDEBAR_MIN_WIDTH = 400` (`sidebar-layout.ts:1`). For any `originalWidth ∈ [400, 415]`, `originalWidth - 16` clamps to `400`, `400 !== originalWidth - 16`, and the pulse returns `skipped/clamped` before the first `setSidebarWidth`. The divider never moves, the terminal never reflows, and the workaround is dead on every attach for as long as the sidebar stays narrow.

End-to-end confirmation (user, running build): with a narrow sidebar the divider never moved and the repaint artifact appeared on every session attach; widening the sidebar past 416 px restored both divider movement and correct rendering. No commit after #1542 (`0bce435`, 2026-08-25) touched the pulse: `src/main/App.tsx` and `src/terminal/components/TerminalView.tsx` are byte-identical between `0bce435` and `576094a` (dispatch-verified).

Call-site context (`TerminalView.tsx:1291-1346`): `settleAttachViewport` requests the pulse only when `snapshot.data.length > 0` (`TerminalView.tsx:1299-1300`), then fits, confirms the final PTY resize, and stores the trace in `layoutPulseTraces` only when the resize was actually sent. The pulse is awaited before the final fit; a `skipped/clamped` pulse does not block the attach, it merely skips the workaround.

## 4. In-scope and out-of-scope

In scope — exactly two files:

1. `src/main/App.tsx` — direction-aware nudge selection and direction-dependent expanded-leg predicate inside `runPulse`; one new private type alias. This is the only product-code change.
2. `src/main/App.sidebar-width.test.tsx` — one existing test reworked (its premise stops holding), three new pulse tests added.

Out of scope, binding on the implementer — NO changes to:

- `src/shared/sidebar-layout.ts` (clamp and the four width constants stay byte-identical; the fix must work *through* the clamp, not by changing it).
- `src/shared/types.ts` — `MainTerminalLayoutPulseTrace` keeps `version: 1` and the exact field set; `MainTerminalLayoutPulseReason` keeps its union; `"clamped"` keeps its meaning ("no headroom in either direction").
- `src/terminal/components/TerminalView.tsx` — pulse request/sample/settle flow unchanged.
- `src/shared/automation-bridge.test.ts` and `src/terminal/components/TerminalView.attachment.test.tsx` — untouched; both consume the trace shape which this plan does not change (they are regression nets proving the shape must not change).
- Any Rust, IPC, CLI, persistence, settings schema, event, or configuration surface.
- `SIDEBAR_PULSE_DELTA_PX`, `SIDEBAR_PULSE_DWELL_MS`, `SIDEBAR_PULSE_LEG_TIMEOUT_MS`, `SIDEBAR_PULSE_REQUEST_TIMEOUT_MS` values (all four stay `16`, `200`, `2000`, `8000`).

## 5. Decided solution

### 5.1 Semantics (normative)

Direction selection in `runPulse`, replacing the current single-candidate clamp check:

1. Compute the inward candidate `clampMainSidebarWidth(originalWidth - SIDEBAR_PULSE_DELTA_PX, window.innerWidth)`. If it equals `originalWidth - SIDEBAR_PULSE_DELTA_PX`, choose direction `inward` with that candidate. This preserves today's behavior byte-for-byte for every width where today's pulse ran.
2. Otherwise compute the outward candidate `clampMainSidebarWidth(originalWidth + SIDEBAR_PULSE_DELTA_PX, window.innerWidth)`. If it equals `originalWidth + SIDEBAR_PULSE_DELTA_PX`, choose direction `outward` with that candidate.
3. Only when both candidates clamp (no headroom of at least 16 px in either direction — e.g. `window.innerWidth ≤ 700` with the sidebar at 400) do `finishPulse(owner, "skipped", "clamped")` and return, exactly as today, before any width write.

The nudge is always exactly `±SIDEBAR_PULSE_DELTA_PX` from `originalWidth`, always through the clamp, and the pulse still never persists the temporary width (it writes the signal directly, exactly as today; `persistWidth` remains user-action-only).

Expanded-leg assertion semantics (normative):

- Direction `inward` (sidebar narrower): require `sample.hostWidth > originalGeometry.hostWidth`, `sample.cols > originalGeometry.cols`, `sample.rows === originalGeometry.rows` — today's predicate, byte-for-byte.
- Direction `outward` (sidebar wider): require `sample.hostWidth < originalGeometry.hostWidth`, `sample.cols < originalGeometry.cols`, `sample.rows === originalGeometry.rows`.
- In both directions the leg still requires a fresh two-fit acknowledgment: `ack && ack.epoch > expansionBaselineObservedEpoch && sameLayoutGeometry(ack.first, sample) && sameLayoutGeometry(ack.second, sample)`. Strict inequalities in both directions: a same-geometry ack (the "same-width event-only observer delivery" case) must never qualify.
- The dwell leg (`waitForPulseDwell` with `expectedGeometry = expandedGeometry`), the restore leg (`sameLayoutGeometry(sample, originalGeometry)`), the restore baseline epoch, the final `sidebarPaneRef.style.width` check, `finishPulse`'s temporary-width restore, `cancelPulseForMutation`, the watchdog, and every `stop`/timeout path are direction-independent and stay byte-identical.

### 5.2 Exact code change 1 — new type alias

Insert after the `SidebarPulseWait` type block (`App.tsx:50-54`, closing `};` at line 54) and the blank line 55, immediately before `type SidebarPulseOwner = {` at line 56:

```ts
type SidebarPulseDirection = "inward" | "outward";
```

### 5.3 Exact code change 2 — direction-aware nudge selection

Replace `App.tsx:510-518` (quoted in `## 3`) with:

```ts
      const inwardCandidate = clampMainSidebarWidth(
        originalWidth - SIDEBAR_PULSE_DELTA_PX,
        window.innerWidth,
      );
      let direction: SidebarPulseDirection;
      let nudgedWidth: number;
      if (inwardCandidate === originalWidth - SIDEBAR_PULSE_DELTA_PX) {
        direction = "inward";
        nudgedWidth = inwardCandidate;
      } else {
        const outwardCandidate = clampMainSidebarWidth(
          originalWidth + SIDEBAR_PULSE_DELTA_PX,
          window.innerWidth,
        );
        if (outwardCandidate !== originalWidth + SIDEBAR_PULSE_DELTA_PX) {
          finishPulse(owner, "skipped", "clamped");
          return;
        }
        direction = "outward";
        nudgedWidth = outwardCandidate;
      }
      owner.nudgedWidth = nudgedWidth;
```

Everything from `const expansionBoundary = ...` onward is unchanged: the original geometry capture, `owner.trace.original`, the baseline epoch, `setSidebarWidth(nudgedWidth)`, `owner.ownsTemporaryWidth = true`.

### 5.4 Exact code change 3 — direction-dependent expanded-leg predicate

Replace the `waitForPulseLeg` match predicate (`App.tsx:546-557`) with:

```ts
        (sample) => {
          const ack = sample.completedObserverAck;
          return Boolean(
            ack &&
              ack.epoch > expansionBaselineObservedEpoch &&
              sameLayoutGeometry(ack.first, sample) &&
              sameLayoutGeometry(ack.second, sample) &&
              (direction === "inward"
                ? sample.hostWidth > originalGeometry.hostWidth &&
                  sample.cols > originalGeometry.cols
                : sample.hostWidth < originalGeometry.hostWidth &&
                  sample.cols < originalGeometry.cols) &&
              sample.rows === originalGeometry.rows,
          );
        },
```

The `onMatch` phase recording, timeout handling (`failed/expanded_timeout`), the dwell leg, the restore leg, and the final checks stay byte-identical. `direction` is a `runPulse`-local captured by the leg closure; nothing else in the file needs it.

### 5.5 Trace shape: deliberately unchanged

`owner.trace.expanded` records the nudged-leg phase in both directions. For an outward nudge `expanded.sidebarWidth` (e.g. `416`) is larger than `original.sidebarWidth` (`400`); for inward it is smaller. The direction is therefore recoverable from the existing fields, the automation consumers (`UiTerminalAutomationTarget.layoutPulse`, `automation-bridge.test.ts:672-723` fixture, `TerminalView.attachment.test.tsx` expectations) all assert the existing shape, and changing the trace (adding a direction field) would force a `version` bump and churn on a diagnostic-only surface for zero functional gain. Do NOT change `MainTerminalLayoutPulseTrace`, `MainTerminalLayoutPulsePhaseTrace`, or `MainTerminalLayoutPulseReason`.

## 6. Alternatives considered and rejected (binding)

1. **Lower `MAIN_SIDEBAR_MIN_WIDTH`** — REJECTED. It moves the dead band instead of removing it: a floor of `384` would re-create the identical failure on `[384, 399]`. It also changes a product-wide layout invariant: drag/keyboard/`Home`/`End` clamping, `aria-valuemin`, window-resize clamping, and persisted-width semantics all read the same constant, and the sidebar UI was sized for the current minimum. Blast radius far beyond the terminal workaround for zero structural gain.
2. **Make the delta adaptive to the actual headroom** (shrink by `originalWidth - 400` when below 16) — REJECTED. A sub-16 px sidebar change can produce zero measurable host-width change after subpixel rounding and zero `cols` change after fit rounding, so the strict two-fit leg could never match and the pulse would fail with `expanded_timeout` (2 s, `failed`) instead of skipping cleanly — strictly worse failure behavior, and nondeterministic per attach. The fixed 16 px delta is what makes the geometry-change assertion sound; keeping it fixed and choosing a direction is the smallest change that preserves that soundness.
3. **Let the pulse bypass the clamp for its temporary width** (nudge below 400) — REJECTED. It would write a width outside the invariant domain that every other write site (drag, keyboard, `Home`/`End`, window resize, programmatic width event, settings load) clamps into, so a concurrent `onWindowResize` would instantly clamp it back and cancel the pulse (`width_changed`), and the rendered divider would disagree with `aria-valuemin`/`aria-valuemax`. The outward nudge achieves the same forced reflow strictly inside the invariant.
4. **Add a `direction` field to the pulse trace** — REJECTED per `## 5.5`.

## 7. Edge cases and failure behavior (binding)

| Case | Behavior |
|---|---|
| `originalWidth ∈ [416, upper]` | Direction `inward`; byte-identical to today (covers the default `440`). |
| `originalWidth ∈ [400, 415]`, headroom ≥ 16 upward | Direction `outward` by exactly 16; full leg/dwell/restore cycle; `completed/completed`. |
| `originalWidth = 400`, `window.innerWidth ≤ 700` (upper bound = 400) | Both candidates clamp; `skipped/clamped` before any width write; divider does not move; no mutation. Identical observable behavior to today. |
| `window.innerWidth = 701-715` (upper = 401-415, sidebar at 400) | Outward candidate clamps to the upper bound ≠ `400 + 16` → `skipped/clamped`. Accepted: headroom < 16 px cannot produce the required reflow; correct per the chosen design. |
| Fractional widths (e.g. `405.5`) | Candidate equality is exact; `405.5 → 421.5` outward. No rounding anywhere. |
| Outward leg never acknowledges (2 s) | `failed/expanded_timeout`; `finishPulse` restores `originalWidth` via the existing `ownsTemporaryWidth` path (`sidebarWidth() === nudgedWidth` → `setSidebarWidth(originalWidth)`); identical to today's timeout semantics. |
| Outward leg sees a same-geometry or opposite-direction ack | Predicate rejects (strict inequalities); pulse keeps polling until timeout/cancel; `width` stays `nudgedWidth`. Mirrors today's rejection of same-width event-only deliveries. |
| User mutation mid-pulse (drag, keyboard, resize, programmatic, side switch) | `cancelPulseForMutation` compares against `owner.nudgedWidth` — direction-agnostic; unchanged. |
| Stale/busy/dragging/persistence-owner/teardown skips | All pre-`runPulse` gates unchanged; the `clamped` skip is the only skip whose trigger set changes (it shrinks). |
| Invalid numbers / corrupt samples | `failPulseForInvalidNumbers` / `failed/exception` paths unchanged. |

## 8. Tests (all in `src/main/App.sidebar-width.test.tsx`)

Test harness facts the new tests rely on (already present): `window.innerWidth = 1400` default in `beforeEach`; `main-sidebar-width-change` event drives the sidebar to a clamped width; `installManualFrames` + `flushFrame` (16 ms/frame) drive the pulse legs; `signalControl` is not needed for these tests.

**T1 — REWORK existing `"skips an exact clamped width without mutation"` (lines 395-410).** Its premise (width 400 at `window.innerWidth = 1400` clamps) stops holding — at 1400 the pulse now nudges outward. Rework it to the only remaining no-headroom configuration: after `renderMain()` + `flushPromises()`, set `window.innerWidth = 700` (same `Object.defineProperty` style as `beforeEach`), dispatch `main-sidebar-width-change` with `{ width: 400 }`, then dispatch a pulse with `sample(800, 80, 2, ack(2, 800, 80))`. Assert: `complete` called once with `{ status: "skipped", reason: "clamped" }`; `sidebarWidth(rendered.root) === "400px"`; `dependencies.settingsUpdate` never called; `frames.pending() === 0`. Rename the test to reflect the new premise (e.g. `"skips with clamped when neither direction can move, without mutation"`).

**T2 — NEW `it.each([400, 415])`: `"nudges outward at narrow widths, rejects inward-direction evidence, and restores exactly"`.** For each width `w`:

1. `renderMain()`, `flushPromises()`, dispatch `main-sidebar-width-change` `{ width: w }` → sidebar `"${w}px"`.
2. `let current = sample(800, 80, 2, ack(2, 800, 80))`; dispatch pulse → `accepted === true`; `sidebarWidth(rendered.root) === "${w + 16}px"`; `complete` not called.
3. Set `current = sample(816, 82, 3, ack(3, 816, 82))` (the geometry an *inward* nudge would produce — grown host and cols, fresh epoch, clean two-fit ack); `flushFrame()` → `complete` still not called (outward leg must reject growth); sidebar still `"${w + 16}px"`.
4. Set `current = sample(784, 78, 3, ack(3, 784, 78))` (narrower host and cols, matching the outward nudge); `flushFrame()`; then 14 dwell frames → sidebar restored to `"${w}px"`.
5. `flushFrame()` with current unchanged (restoration must not be acknowledged by the expansion ack) → not completed; set `current = sample(800, 80, 4, ack(4, 800, 80))`; `flushFrame()` → `complete` called exactly once with:
   - `status: "completed"`, `reason: "completed"`, `trace.version: 1`, `trace.settingsWritesDelta: 0`;
   - `trace.original = { sidebarWidth: w, hostWidth: 800, cols: 80, rows: 24 }`;
   - `trace.expanded = { sidebarWidth: w + 16, hostWidth: 784, cols: 78, rows: 24, baselineObservedEpoch: 2, completedObserverAck: ack(3, 784, 78) }`;
   - `trace.restored = { sidebarWidth: w, hostWidth: 800, cols: 80, rows: 24, baselineObservedEpoch: 3, completedObserverAck: ack(4, 800, 80) }`;
   - `dwellMs >= 200 && dwellMs <= 8000`; `dependencies.settingsUpdate` never called; `frames.pending() === 0`.

**T3 — NEW: `"uses the exact expanded leg timeout after an outward nudge and restores the original width"`.** Mirror the existing `"uses the exact expanded leg timeout and clears its frame, watchdog, and ownership"` test (fake timers + manual frames): sidebar to 400 via `main-sidebar-width-change`; dispatch pulse with `sample(800, 80, 1, ack(1, 800, 80))`; assert sidebar `"416px"`; `advanceTimersByTimeAsync(1999)` → not completed; `+1` → completed once with `{ status: "failed", reason: "expanded_timeout" }`; sidebar restored to `"400px"`; `frames.pending() === 0`; `vi.getTimerCount() === 0`.

No other existing test changes. In particular the inward-path tests (`"requires post-boundary two-fit acknowledgements..."`, the timeout tests at width 440, the mutation-cancel tests) must pass untouched — they are the regression net proving the inward path is byte-identical.

## 9. Acceptance criteria and verification

The implementation reviewer (Step 6) and the Step 7 certification re-run these on the exact final branch head:

1. `git diff 576094a...HEAD --stat` shows exactly `src/main/App.tsx` and `src/main/App.sidebar-width.test.tsx` changed (plus the plan file only if the tech lead required force-adding it); no other tracked or ordinary-untracked files.
2. `npx tsc --noEmit` exits 0 (repository typecheck; CI runs the same as `npm run typecheck`).
3. `npx vitest run src/main/App.sidebar-width.test.tsx` — all tests green, including T1 rework, T2 both widths, T3, and every pre-existing pulse test untouched.
4. `npm run test:debt` — no new findings (T1-T3 are real tests; no `.skip`, no placeholder bodies).
5. `npm test` locally — green, or failing only with the known #480 unhandled WebSocket rejection signature tolerated by `scripts/classify-test-run.mjs` (the CI gate's whitelist).
6. PR exact-head CI, GitHub-authoritative: `frontend-regression` (typecheck + `npm test` under the #480 guard), `test-debt` (including the classifier self-checks), `validate-branch-name`, and every other job `pr-regression-gates.yml` runs for the PR — the Rust/cargo matrix, `rust-fmt`, `terminal-snapshot-portable`, `windows-release-cli-smoke`, plus `lockfile-check`/`version-sync-check`/`bundle-validation` if their triggers fire. A frontend-only diff cannot change the Rust jobs' inputs, but exact-head success on all configured-required checks is still the delivery gate.
7. Objective product behavior: with the sidebar at 400 px on a window ≥ 716 px wide, a session attach moves the divider 400 → 416 → 400 with a ≥ 200 ms dwell and completes the pulse (trace `status: "completed"`); with the sidebar at 440 px the behavior is indistinguishable from pre-fix builds; with a ≤ 700 px window and sidebar at 400 px the pulse returns `skipped/clamped` without moving the divider (unchanged).

## 10. Compatibility impact

- No IPC, event, CLI, persistence, settings-schema, or Rust surface changes; no new dependencies; no new modules; `package.json`/lockfiles untouched.
- `MainTerminalLayoutPulseTrace` stays `version: 1` with the exact same fields; the `expanded` phase may now carry a `sidebarWidth` larger than `original.sidebarWidth` — consumers (`TerminalView.tsx` trace storage, `UiTerminalAutomationTarget.layoutPulse`, automation-bridge) treat it as an opaque diagnostic and are unaffected.
- `clamped` reason: meaning narrows from "inward nudge was clamped" to "no 16 px headroom in either direction". Consumers treat reasons opaquely; no code reads `"clamped"` for behavior.
- Persisted sidebar width semantics unchanged: the pulse never persists; user drag/keyboard writes and their 500 ms debounced save are untouched.

## 11. Delivery nonfunctional invariants (gates, per `skills/delivery-nonfunctional-invariants`)

Accepted task class and threat model: **routine frontend defect fix**; accepted threat model = repository-pinned toolchain (CI: Node 22 via `actions/setup-node@v5`, `npm ci`, npm 11.6.2 pinned in every job) + GitHub CI as the authoritative host execution evidence. No enhanced controls applicable: no release/signing/packaging provenance, no untrusted host, no security boundary, no destructive migration, no persistence schema change — each named hazard absent, so every enhanced control is explicitly non-applicable; baseline controls suffice.

1. **CI-to-plan parity**: derived from `.github/workflows/pr-regression-gates.yml` (jobs `test-debt`, `rust-regression` ×3 OS legs, `rust-fmt`, `terminal-snapshot-portable`, `windows-release-cli-smoke`, `frontend-regression`; Node 22, npm 11.6.2). Locally reproducible subset: typecheck, `vitest` single-file run, full `npm test`, `test:debt`, `npm run build` (vite). Remote-only (GitHub-hosted cargo matrix, Tauri build steps): owned by CI; acceptance rule is success of every triggered and configured-required check at the exact PR-head SHA; evidence from another SHA or a skip does not satisfy the gate.
2. **Deterministic toolchain and build**: Node 22 + `npm ci` (lockfile `package-lock.json` untouched by this plan); explicit commands in `## 9`; no wrapper/provenance requirements — outside the accepted threat model.
3. **Authorized, traceable Git**: open issue #1643; branch `fix/1643-sidebar-pulse-clamped-at-min-width` created from the frozen base `576094a` (verified clean at authoring); all state-changing Git inside `repo-AgentsCommander`; deliver via PR to `main`, never direct push. Before the first product mutation and again before PR creation/update: `git fetch origin main`, classify drift between `origin/main` and `576094a` by changed paths and semantic relevance. Drift touching `src/main/App.tsx`, `src/main/App.sidebar-width.test.tsx`, frontend test tooling, or formatter/lint configuration requires refreshing the affected evidence and re-review; drift unrelated to this change is recorded and synchronized at the next bounded gate and must not restart the design or move the base.
4. **Process state and working directory**: run all commands from the repo root with the repository's `node_modules` (`npm ci` if absent); no inherited config that changes the commands is known; task-created state (if any) confined to gitignored `node_modules`/`plans/` and standard Vite/Vitest temp locations; final `git status` must show only the intended paths.
5. **Validation and scope before acceptance**: frozen base and intended path set in `## 4`; changed-path postcondition in `## 9.1`; compare final tracked, staged, ordinary-untracked, and lockfile state against `## 4` before reporting done.
6. **Mutation ownership and no-clobber recovery**: the change is two files; before writing, recheck branch/base/index and the two files' state; on failure, restore only paths this run actually changed and only while their current state is demonstrably this run's output (scoped `git restore -- <file>` for those two paths; never repository-wide reset). After success, prove the intended path set and clean status.
7. **Bounded execution and durable diagnostics**: vitest/tsc run non-interactively under the standard runner with default timeouts; on failure keep the full stdout/stderr and the failing test name; a failed command is never reported as success.
8. **Evidence discipline**: zero is a valid state (e.g. `test:debt` finding count 0, empty `git status` delta beyond the two files, no `settingsUpdate` calls during pulses — T1-T3 assert these); evidence comes from the executable commands in `## 9` with expected results stated; what cannot run locally (GitHub-hosted cargo/Tauri legs) is assigned to CI with the exact-head acceptance rule.

## 12. Dependency-cycle and layering gate (per `skills/verify-no-dependency-cycles`)

The plan adds **zero module-to-module references**: `src/main/App.tsx` gains one local type alias and two in-function code blocks using only already-imported bindings (`clampMainSidebarWidth`, `SIDEBAR_PULSE_DELTA_PX`); the test file changes only local test code. No new `import`/`export`, no new module, no moved symbol, no cross-boundary arc, no SCC can change (`cyclicSccs` unchanged, member sets identical, arc record `src-tauri/module-arcs.txt` untouched and byte-identical). Role/layering hygiene: no layer gains a UI-transport or transport-taking dependency — the change is entirely inside the existing frontend surface module that already owns the pulse. The implementation reviewer verifies by diff inspection that the changed-path set ( `## 9.1`) contains no `import` additions and no files outside `## 4`.

## 13. Manual smoke (implementer, running build)

On a dev build with a ≥ 716 px window: set the sidebar to 400 px (drag or keyboard to the minimum), attach/reattach a session with buffered output, and confirm the divider performs 400 → 416 → 400 and the repaint artifact does not appear; then widen to 440 px and confirm identical behavior to pre-fix. Optionally repeat with the sidebar at 415 px. This is complementary evidence; the automated gate is `## 9.1-9.6`.

## 14. Partition outcome

Applied `skills/plan-partitioning` (Partition rule) at Step 4: cut by owner (single owner `ac-dev-webpage-ui-v4` — frontend only), by contract (no IPC/CLI/persistence/schema change, no consumer-side phase), by green-tree boundary (the whole change lands as one build/test-green unit), by budget (2 files < 10-file trigger). The partition trigger does not apply.

**PARTITION: 1 phase.** One self-contained plan file; no `epic.md`, no child phase files.

## 15. Certification report summary

- Scope: `src/main/App.tsx`, `src/main/App.sidebar-width.test.tsx` only; base `576094a`, branch `fix/1643-sidebar-pulse-clamped-at-min-width`, issue #1643.
- Decided solution: direction-aware nudge (inward preferred, byte-identical when possible; outward fallback within the clamp; `skipped/clamped` only when neither direction has 16 px headroom), with the expanded-leg hostWidth/cols comparisons inverted exactly per direction; trace shape, dwell, restore, cancel, watchdog, persistence, and timeout behavior unchanged.
- Rejected: lowering `MAIN_SIDEBAR_MIN_WIDTH` (moves the dead band, product-wide blast radius), adaptive delta (unsound leg assertion, worse failure mode), clamp bypass (invariant violation, concurrent-resize fragility), trace shape change (diagnostic churn, no gain).
- New arcs: none. Layering: unchanged. Partition: 1 phase.
