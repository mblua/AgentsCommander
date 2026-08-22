# Issue #1489: close terminal attach replay ordering and viewport settlement

Status: READY_FOR_IMPLEMENTATION

Repository: `repo-AgentsCommander`

Planning base and investigated HEAD: `45530b46b0a3d4bfe8715ac8b19916df98b6f8cd` (`main == origin/main`, branch `fix/1489-terminal-attach-replay-settlement`; the branch is no longer clean: the preserved Step 8 implementation changes two in-scope files, uncommitted, section 2.1)

Codebase Memory project: `D-0_repos-AgentsCommander_iac-.ac-wg-17-dev-v5-team-repo-AgentsCommander`

Impact-HTML: plans/1489-terminal-attach-replay-settlement-impact.html

## 1. Issue and objective

Issue: https://github.com/mblua/AgentsCommander/issues/1489 (parent epic #1478; absorbs #1490, #1491, #1492). **Full planning restart**: the prior certification (`READY_FOR_IMPLEMENTATION`, plan hash `0DB8084612431756D84CD153233FC57DEC7BAD697B6A94F952469BB7D75AC75A`, HTML hash `39DC567CB95A5754FA3E7AA7D63DD445C80A01412D906064780C28AD1780CF1D`) was invalidated by fresh implementation evidence: the Step 8 cold implementation proved the exact two-file scope insufficient because two existing harness fakes outside that scope do not satisfy the plan-mandated production contract (section 2.6). This revision started as the Step 4 draft for the proven smallest scope: exactly four existing files; it was enriched by dev (Step 5, round 2, report `20260822-070714`) and Grinch (Step 6, round 2, report `20260822-071549`), adjudicated at the architect Step 7 consensus (round 2, sections 9.3 and 12), and certified `READY_FOR_IMPLEMENTATION`. No code implementation or edit of application or test code is part of this plan task.

Objective: close the reported terminal-attach regression as one atomic frontend transaction in exactly four existing files (one production file, three test files):

1. Make replay-ordering and viewport-settle failures deterministic through the existing attachment test harness, extended with asynchronous xterm write completion and observable viewport state.
2. Fence snapshot replay before retained live bytes: snapshot bytes complete parsing first, every retained live byte follows exactly once, and stale attach/detach generations cannot mutate a newer replay.
3. Settle the attached viewport to the current screen exactly once, only after replay/parser completion and the fit boundary; never bottom-scroll ordinary live writes.
4. Leave active end-to-end regression coverage for attach, detach/reattach, switch-away/back, stale generations, intentional scroll-up preservation, meaningful multi-viewport history, near-64-KiB ring history, and the Codex and Pi lifecycles.
5. Make the two existing harness fakes that share the xterm contract compatible with the frozen production mechanism, changing no assertion and no product behavior: `TerminalView.render-gate.test.tsx` and `App.workflow.test.tsx` (section 4.8).

The final behavior must preserve snapshot bytes followed by live bytes exactly once, and must preserve an intentional user scroll-up during ordinary live output.

## 2. Evidence and identified cause

### 2.1 Verified setup

- Clean branch `fix/1489-terminal-attach-replay-settlement` created from `main == origin/main == 45530b46b0a3d4bfe8715ac8b19916df98b6f8cd`; HEAD is still the base: the Step 8 cold implementation produced **no commit** and left exactly two ordinary working-tree paths modified, both in scope below (`TerminalView.tsx`, `TerminalView.attachment.test.tsx`); nothing staged, no stash. Those preserved changes are plan-faithful (audited symbol-by-symbol against this plan's sections 4 and 5.1/5.2 in the Step 8 blocker report `20260822-061610`) and must not be edited, reset, restored, staged, or committed by this planning revision.
- #1489 is the only delivery child; #1490-#1492 are closed `NOT_PLANNED`; #1478 remains the coordination parent.
- Unique primary production file: `src/terminal/components/TerminalView.tsx` (900 lines at the clean base; now 1187 lines with the preserved uncommitted implementation; line references in sections 2.2 and 4 are against the clean base, while every symbol line reference in section 5.1 is re-checked against the preserved working-tree source and exact).
- Exclusive supporting verification surface: `src/terminal/components/TerminalView.attachment.test.tsx` (9 tests at the clean base; 17 tests in the preserved working tree, all green).
- The legacy `fix/1478-terminal-attach-replay-ordering` plan/HTML are superseded diagnosis evidence only; no part of them is reused as plan, HTML, digest, approval, or implementation authority.

### 2.2 The single causal attachment transaction (cold specialist audit, re-verified against source)

`src/terminal/components/TerminalView.tsx` owns the whole attach chain:

1. `transitionAttachment` (lines 696-753) serializes transitions on `attachChain`, re-checks `desiredSessionId` after every await, and awaits `TerminalOutputAPI.attachOutput(target)`.
2. While the seed is in flight, `beginSeed` (632-654) sets `snapshotReplayPending = true`, and `writeLivePtyOutput` (478-491) retains every live event (`retainForSnapshotReconcile`, 466-476) AND writes it live through `writeTerminalBytes` (127-131), which calls `entry.terminal.write(data)` with **no completion callback**.
3. On resolution, `settleSeed` (656-668) -> `concludeSnapshotFetch` (514-534) -> `applySnapshot` (570-628) -> `rebuildFromSnapshot` (557-568): `terminal.reset()`, `lastAppliedSequence = snapshot.sequence`, write snapshot bytes, then `flushPendingEvents` (493-500) replays the retained events. There is **no xterm write fence**: nothing proves the snapshot bytes were parsed before the retained bytes are replayed, and nothing proves either completed before the delayed fit/resize.
4. `applySnapshot` schedules `scheduleViewportSync` (381-395) on two animation frames (fit + `sendPtyResize`) with no parse-completion barrier, and the attach path has **no one-shot bottom settlement**: `scrollToBottom` is never called anywhere in the file.
5. There is no attach-generation guard: no async continuation in the attach transaction validates that the capture that started it is still the current one.

### 2.3 Grounded xterm 6.0.0 semantics (verified in `node_modules/@xterm/xterm/lib/xterm.mjs` and `typings/xterm.d.ts`)

- `write(data, callback?)`: data enters a FIFO queue (`_writeBuffer`/`_callbacks`) and is parsed asynchronously; the callback fires only after the chunk was **parsed and applied**. The typings state: "This callback must be provided and awaited in order for buffer to reflect the change in the write." This is the only frontend-visible parse-completion signal.
- `Terminal.reset()`: resets the input handler, buffer service, charset, core, and mouse state, but **does not clear the pending write queue**; bytes queued before a reset are still parsed after the reset, into the fresh buffer.
- `scrollToBottom()`: `scrollLines(ybase - ydisp)`; the settled position is `viewportY == baseY` on `buffer.active`. After a `reset()` the fresh buffer has `ydisp == ybase == 0`, and output writes while `ydisp == ybase` keep the view pinned to the bottom, so a replay without user input ends at `viewportY == baseY` and the settle's `scrollToBottom` is a no-op.
- `write()` throws **synchronously** with `"write data discarded, use flow control"` when the unparsed queue exceeds 50 MiB (`_pendingData > _c = 5e7`), **before** queueing the chunk; the frontend must treat that throw as "the byte was never queued" (drain release, section 4.2), never as a pending write.
- The parse loop (`_innerWrite`) time-slices at ~12 ms and continues across per-chunk parse rejections (xterm rethrows the rejection as an unhandled microtask error and advances); a **callback that itself throws aborts the loop and wedges the remaining queue** (nothing reschedules it), so every callback this plan passes to `write` must be throw-free by construction (section 4.2). Terminal `dispose()` does not cancel the loop's scheduled continuation, so queued chunks still complete after disposal.
- `createTerminalOptions` (`src/terminal/components/terminal-options.ts`) sets `scrollback: 10000`, so replay history up to and beyond the near-64-KiB ring is retained in the xterm buffer.

### 2.4 Proven defect mechanics

Because `writeLivePtyOutput` writes retained events live **while the seed is pending**, those bytes are queued in xterm **before** the reset+snapshot that `rebuildFromSnapshot` performs when the seed resolves. The reset does not discard that queue. On a real xterm the final screen order becomes: retained live bytes (parsed after the reset, before the snapshot), then snapshot bytes, then the retained bytes replayed by the flush. Live output therefore **straddles the snapshot and is duplicated**, and the visible result is a malformed replay whose exact shape depends on queue timing: a nondeterministic regression. The current fake terminal hides this entirely because its `reset()` synchronously clears `screen`, modeling a queue discard that real xterm does not perform.

Independently, the attach path never bottoms: after replay and the delayed fit/resize there is no guarantee the viewport shows the current screen (reported blank-bottom and off-bottom cases for Codex and Pi lifecycles), and no deterministic harness state (`viewportY`/`baseY`) can observe either failure because the fake lacks buffer metrics and a `scrollToBottom` counter.

Once a write fence and a settle step are added, any callback or animation-frame continuation from an older attach must be provably inert against a newer replay; today no generation identity exists to prove that.

### 2.5 Boundary evidence (preserved properties)

- The backend activation boundary (`activate_terminal_output` snapshot + attachment registration under the parser mutex) is untouched: this plan changes no Rust.
- The frontend already retains live events during snapshot acquisition and filters them by sequence (`shouldDropAlreadyAppliedEvent`, watermark `lastAppliedSequence`). Both properties are preserved.
- The #961 seed contract ("the seed is a seed, not a gate"; live bytes are never gated behind the seed) is preserved: live bytes still reach xterm on arrival; the fence orders the *replay* relative to the *snapshot write*, it does not gate live output behind the seed.

### 2.6 Blocker evidence: the two-file scope is proven insufficient (Step 8, fresh implementation evidence)

The Step 8 cold implementation executed the two-file plan faithfully (preserved work assessment in blocker report `20260822-061610`; pre-fix red evidence reproduced deterministically for tests 10 and 11 and recorded at the replica root `scratch/1489-step8/pre-fix-red-evidence.txt`). All in-scope gates passed on the preserved head, but the plan's own full-suite gate fails on **two out-of-scope test files whose existing xterm fakes do not satisfy the new production contract**:

1. **`src/terminal/components/TerminalView.render-gate.test.tsx` (8 tests, 2 failed):** its mocked `Terminal.write(data: unknown): void` (current line 89) never invokes the optional parse-completion callback, and its fake exposes no `buffer`. The plan-mandated drain (`drainPendingWrites`, 4.2) registers `inFlight` for every live write in the reconcile window and awaits the write callbacks; with no callback ever fired, `inFlight` never returns to 0 and the drain awaits forever, so reset+replay never run. Observed: both replay tests (`rebuilds from the snapshot without duplicating live output it already contains`, `replays the live output the late snapshot does not cover`) fail with `AssertionError: expected +0 to be 1` on `terminal.resets` after a 1000 ms `waitFor`. Even if the callback were fired, the fake's missing `buffer` would crash the settle there.
2. **`src/terminal/App.workflow.test.tsx` (30 tests, all pass; 4 unhandled errors):** its fake already fires the optional write callback (current line 97-102, the #1283 admission settles its replay/write gates from it), so the drain releases and the settle runs; but it exposes no `buffer`, and `settleAttachViewport` reads `entry.terminal.buffer.active` (production line 819, the plan's section 4.5 mandated instrumentation), throwing `TypeError: Cannot read properties of undefined (reading 'active')`. The settle is fire-and-forget, so the throw surfaces as four unhandled rejections in the suite's error tally while its 30 assertions pass.

**Observed blocker baseline (fresh runs, recorded verbatim in `scratch/1489-step8/full-suite-blocker-evidence.txt`):**

| Command | Result |
|---|---|
| `npm test -- src/terminal/components/TerminalView.attachment.test.tsx --reporter=default` | **17 passed** (also 6x stable in the prior session) |
| `npm test -- --reporter=default` | **2 failed / 1599 passed (1601); 4 unhandled errors** |
| `npm run typecheck` | PASS |
| `npm run check:frontend-dependencies` | PASS (0 errors, 344 modules, complete-root gate) |
| `npm run build` | PASS (pre-existing warnings only) |

**Proven smallest correction (verified compatible fakes):** `saturation`, `spawn-size`, `App.workgroup-task-race`, and `browser/App.workflow` already satisfy the new contract: `saturation` and `spawn-size` never reach a wedging drain or the settle (saturation's live writes land after the window closes, so nothing registers in the drain, and its fake's missing callback only stalls the flush, which its assertions do not depend on; spawn-size's attaches resolve seedless); `App.workgroup-task-race` fires the callback and exposes a partial `buffer` (`baseY`/`type` read as `undefined`, which does not throw); `browser/App.workflow` never attaches at all (`transitionAttachment` early-returns in browser mode), so no drain or settle can run there. Only the two fakes above are incompatible. The minimal correction is exactly: render-gate's fake must accept and invoke the optional write callback and expose the buffer metrics the frozen mechanism reads; App.workflow's fake must expose those buffer metrics and keep its existing callback behavior. No assertion in any of the three test files changes; no new test, helper abstraction, dependency, production fallback, timeout/deadline, or file is added (section 4.8). The two compatibility corrections must turn the blocker baseline into **1601 green and zero errors**.

## 3. Scope

### In scope (exactly four existing files)

- `src/terminal/components/TerminalView.tsx` (production, the only product file): deterministic write fence, generation guard, one-shot attach viewport settlement, awaitable typed resize, bounded settled instrumentation. No new imports, no new modules, no new files.
- `src/terminal/components/TerminalView.attachment.test.tsx` (exclusive verification harness): deterministic asynchronous xterm write completion (queue semantics matching xterm 6.0.0), observable `viewportY`/`baseY`/`bufferLength`, `scrollToBottom` counter, gated write-completion control, multi-viewport and near-64-KiB fixtures, and the new behavior tests.
- `src/terminal/components/TerminalView.render-gate.test.tsx` (harness-fake compatibility only, the proven blocker correction): its mocked `Terminal.write` accepts and invokes the optional parse-completion callback and its fake exposes the buffer metrics the frozen settle mechanism reads. No assertion changes; no new test.
- `src/terminal/App.workflow.test.tsx` (harness-fake compatibility only): its fake exposes the same buffer metrics; its existing callback behavior (write fires the optional callback) is preserved unchanged. No assertion changes; no new test.

#1489 owns all four files exclusively; no later child issue may reopen any of them for this defect.

### Out of scope

- Any change to `src/terminal/components/terminal-session-registry.ts`, `src/shared/types.ts`, `src/shared/ipc.ts`, `src/shared/transport.ts`, `src/shared/terminal-viewport.ts`, `src/shared/testing/*`, `terminal-options.ts`, `prompt-input-capture.ts`, stores, or any other source/test file.
- New dependencies, abstractions, protocols, IPC, schemas, persistence, Rust/backend work, config changes, new source/test files, generic framework work, or unrelated terminal refactoring.
- Any addition to the two compatibility files beyond the frozen production contract they must satisfy: no new test, no new helper abstraction, no production fallback, no timeout/deadline machinery, no other file.
- Unconditional bottom scrolling on live writes; gating live output behind the seed; changing the seed watermark/reconcile budget constants; diagnostics transport beyond the bounded `console.debug` settle record inside `TerminalView.tsx`.
- Silently fixing a newly discovered defect outside this scope; such a discovery returns the issue to Full planning, recertification, artifact freeze, and purge without a human-approval pause.

If implementation evidence proves the exact four-file scope insufficient, stop and report a blocker; do not broaden it.

## 4. One decided solution

Make the attach transaction a generation-scoped, write-fenced pipeline in `TerminalView.tsx`:

```
attach resolves
  -> drain: await parse completion of every live write queued since beginSeed
     (live bytes were written on arrival; they must be parsed BEFORE the reset)
  -> validate generation + identity + visibility
  -> re-check reconcile budget (may have been abandoned during the drain)
  -> capture retained events; close the reconcile window
  -> reset(); write snapshot bytes with a completion callback
  -> on snapshot parse completion: replay retained live bytes (watermark-filtered),
     exactly once, with the last write carrying the settle trigger
  -> on replay completion: one rAF -> fit() -> await typed pty resize
     -> validate again -> scrollToBottom() at most once (user-scroll guard) -> record settled metrics
```

Every async continuation (write callbacks, drain loop, animation frame, resize continuation) validates `isCurrentAttach`: same entry in the registry, not destroyed, same attach generation, still the desired and attached and visible session. A stale continuation becomes a no-op and can never mutate a newer replay.

Ordinary live output (`writeLivePtyOutput` outside the reconcile window) keeps its exact current behavior: write on arrival, watermark drop, never `scrollToBottom`.

The mechanism is fully contained in the component: generation and drain state live in component-scope `WeakMap`s keyed by the entry object, so `SessionTerminalEntry` and the registry file stay untouched.

### 4.1 Component-scope state (inside `TerminalView`)

- `const attachGenerations = new WeakMap<SessionTerminalEntry, number>();`
- `const attachDrains = new WeakMap<SessionTerminalEntry, AttachDrain>();`
- Component-scope type `interface AttachDrain { inFlight: number; promise: Promise<void> | null; resolve: (() => void) | null; userScrolled: boolean }`, declared beside the file's only existing internal interface `SnapshotSettle` (line 509). The file has no module-level interface today; component scope beside `SnapshotSettle` is the established convention, and the placement is erased at compile time. `userScrolled` is the attach-window user-scroll marker read by the settle (4.5).

`beginSeed` (632-654) additionally: `attachGenerations.set(entry, (attachGenerations.get(entry) ?? 0) + 1);` and a drain **create-if-absent reset**: `const existing = attachDrains.get(entry); if (existing) { existing.userScrolled = false; } else { attachDrains.set(entry, { inFlight: 0, promise: null, resolve: null, userScrolled: false }); }`. The drain is per-**entry**, never replaced and never deleted: `inFlight` counts every byte still queued in this entry's xterm FIFO **across generations** - the three classes this plan's write paths queue: the window-registered live writes of 4.2, the snapshot write of 4.4 step 6, and the fenced replay writes of 4.4 step 4 - because `reset()` does not discard the queue (2.3): a newer generation's fence must also await an older generation's still-unparsed writes, or those bytes would parse into the fresh buffer after the newer reset and before the newer snapshot (an older async write mutating the newer replay, forbidden by the issue's acceptance). The snapshot write registers in the shared drain with the same 0→1 promise replacement, `inFlight` increment, and throw-safe release as 4.2, and its completion callback releases the drain (decrement, resolve at zero) and then runs the flush trigger. The count is complete only because 4.4 registers the snapshot write and every fenced replay write; without those registrations the older snapshot or replay bytes would escape the fence and could still straddle a newer reset (Grinch finding 2). FIFO order then guarantees: when `inFlight` reaches zero, every byte queued before the newest counted write has parsed (post-close live bytes ride ahead of the fenced snapshot and replay writes in the FIFO and are covered by them), so the reset is safe. Boundary of the claim (recorded Step 7 adjudication, out of scope, do not fix): a live byte queued **after** the newest counted write is not covered by that guarantee - it is provably parsed only via xterm's ~12 ms parse slice, so a live byte queued while the session is still visible immediately before a full A→B→A switch that completes in under one slice could parse after the newer reset and ahead of the newer snapshot (impact: at most a few duplicated bytes, already reflected in the newer snapshot, which was captured after the byte was emitted). This is pre-existing (the #961 live-on-arrival contract has always had it), orders of magnitude narrower than the straddle this plan fixes, unmodelable by the harness (the fake applies bytes at queue time and `reset()` clears them, so no deterministic test can observe it), and the only fix (registering post-close live writes unconditionally) would re-open the section 2.6 fake-compat analysis (saturation's fake never fires callbacks and would wedge the drain) and violate the four-file scope. It does not contradict the issue's acceptance, which is about retained-replay exactly-once and stale-generation mutation. The implementer must not add post-close registration or deadline machinery for it. Everything else in `beginSeed` is unchanged.

### 4.2 Write fence

- `writeTerminalBytes` (127-131) gains an optional third parameter `onWritten?: () => void` and becomes: set `hasRenderedOutput`, clear replay status, then `entry.terminal.write(data, onWritten)`. This keeps the single call site invariant (#1355): `terminal.write` still has exactly one call site in the frontend.
- `writeLivePtyOutput` (478-491) keeps its exact signature and return type (the draft's `onWritten` parameter and `boolean` return are dropped: no caller consumes either, and the drain is registered internally). While `snapshotReplayPending` it retains the event as today; the drain is registered **only on the actual write path, after the `shouldDropAlreadyAppliedEvent` drop check**, and only while `entry.snapshotReplayPending` holds (the drain always exists from `beginSeed`; 4.1). A retained event dropped by the watermark (possible during a re-attach, before the reset: `lastAppliedSequence` still holds the previous attach's watermark) never writes, so it must never increment `inFlight`; counting it would hang the drain and the replay with it. Registration shape: if `drain.inFlight === 0`, replace the promise (`drain.promise = new Promise<void>((resolve) => { drain.resolve = resolve; });`), then `drain.inFlight += 1`; the write passes an `onWritten` that decrements `inFlight` and resolves the drain when it reaches zero. The 0→1 replacement is load-bearing: a write arriving between a drain resolve and the drain loop's continuation must present a fresh, unresolved promise for the loop to await. The watermark drop, write, and `markAppliedSequence` behavior are unchanged. **Throw-safe write**: every `writeTerminalBytes` call this plan makes  -  the live-path call here, the snapshot write in 4.4, and each fenced replay write in 4.4  -  is wrapped in `try`/`catch`; on a synchronous throw from `terminal.write` (2.3: the 50-MiB queue guard throws before queueing) the drain registration (if any) releases first: the same `onWritten` runs (the byte was never queued, so the drain must not wait for it). On the live path the error is rethrown exactly as today's propagation; in `replaySnapshotFenced` and `flushPendingEventsFenced` (fire-and-forget continuations, where a rejection would be unhandled) the error is logged with `console.warn("[terminal] attach replay " + sessionId + " failed:", error)` and that generation's pipeline stops (live output continues; the next attach re-seeds)  -  never a rethrow (Grinch finding 1). **Throw-safe callbacks**: every other callback this plan passes to `write` (snapshot callback in 4.4, flush `finalize`) is wrapped so it cannot throw  -  a callback that throws aborts xterm's parse loop and wedges the remaining queue (2.3); a synchronous failure inside a replay write is caught, logged with `console.warn("[terminal] attach replay " + sessionId + " failed:", error)`, and stops that generation's pipeline (live output continues; the next attach re-seeds).
- New `drainPendingWrites(entry, sessionId, generation): Promise<void>`: if no drain exists, return. Otherwise loop `while (drain.inFlight > 0) { await drain.promise; if (!isCurrentAttach(...)) return; }`. A write arriving between a resolve and the continuation increments again and creates a fresh promise (0→1 replacement in the write path), so the loop re-awaits; the loop is correct under interleaving because increments and decrements are synchronous inside their own callbacks, and the exit check plus the caller's continuation (close → reset → snapshot write) run in one synchronous block, so no event callback can land between the final check and the snapshot write.
- New `closeSnapshotReconcile(entry)`: `snapshotReplayPending = false`, `pendingSnapshotEvents = []`, `pendingSnapshotBytes = 0`. It does **not** delete the drain: the drain must keep fencing this entry's queued writes for the next generation (4.1), and the write path's `snapshotReplayPending` gate already prevents any post-close registration.

### 4.3 Reconcile-window lifecycle

- `concludeSnapshotFetch` (514-534) keeps validation (`registry.get(sessionId) === entry`, `!entry.destroyed`), timer clearing, and settle construction, but **stops clearing** `snapshotReplayPending`, `pendingSnapshotEvents`, and `pendingSnapshotBytes`; consumers now close the window explicitly.
- `failSeed` (670-688) calls `closeSnapshotReconcile(entry)` after its warning (no reset ran, so nothing is replayed; the window must still close).
- `applySnapshot` (570-628): the seedless branch (`!snapshot || snapshot.data.length === 0`) calls `closeSnapshotReconcile(entry)` and keeps its existing `scheduleViewportSync` resync (test 9 depends on it); the discard branch (`entry.hasRenderedOutput && !settle.reconcilable`) calls `closeSnapshotReconcile(entry)` before returning.

### 4.4 Fenced replay

- `applySnapshot` snapshot path: after `lastSentViewport = null`, capture `const generation = attachGenerations.get(entry) ?? 0;` (safe at this point: see the invariants in 4.7), apply the grid resize via `resizeTerminalForSnapshot` exactly as today, then call `void replaySnapshotFenced(sessionId, entry, snapshot, generation);` and **remove** the `scheduleViewportSync` call from this path (the settle sequence replaces it; the seedless path keeps it).
- New `replaySnapshotFenced(sessionId, entry, snapshot, generation)` replaces `rebuildFromSnapshot`:
  1. `await drainPendingWrites(entry, sessionId, generation);`
  2. `if (!isCurrentAttach(sessionId, entry, generation)) return;`
  3. Re-check the reconcile budget: `if (!entry.snapshotReplayPending) { console.warn("[terminal] snapshot " + sessionId + " discarded: live output outran the reconcile budget"); return; }` (the retention may have been abandoned during the drain; today the same discard happens at apply time).
  4. `const retained = entry.pendingSnapshotEvents;` then `closeSnapshotReconcile(entry);`
  5. `entry.terminal.reset(); entry.hasRenderedOutput = false; entry.lastAppliedSequence = snapshot.sequence;`
  6. `writeTerminalBytes(entry, new Uint8Array(snapshot.data), () => { if (!isCurrentAttach(sessionId, entry, generation)) return; flushPendingEventsFenced(entry, retained, sessionId, generation, snapshot); });` (the snapshot is threaded through so the settle can record the snapshot-grid and seed-size evidence; every function in this chain is new, so no existing signature changes). The snapshot write **registers in the shared drain (4.1) with the same shape as the replay-write registration in step 4** - the 0→1 promise replacement, the `inFlight` increment, and the throw-safe release (4.2) - composing Grinch findings 1 and 2: on a synchronous throw from `terminal.write` (2.3) the drain releases with the 4.2 `"[terminal] attach replay ... failed:"` warn and this generation's pipeline stops, never an unhandled rejection (the pre-fix `rebuildFromSnapshot` ran inside the attach continuation's `try`/`catch` and degraded gracefully into `failSeed`; the fenced pipeline must keep that property). Its completion callback is wrapped per 4.2 (it cannot throw into xterm's parse loop): it first releases the drain (decrement, resolve at zero) and, if the generation is still current, then runs the flush trigger (`flushPendingEventsFenced(entry, retained, sessionId, generation, snapshot)`). This registration makes a later generation's fence await an older generation's still-queued snapshot bytes before its reset - without it, a fast re-attach could reset while the older snapshot bytes are still in the FIFO, and those bytes would parse into the fresh buffer ahead of the newer snapshot (the snapshot-class variant of Grinch finding 2, exercised by test 12's main body).
- New `flushPendingEventsFenced(entry, events, sessionId, generation, snapshot)` replaces `flushPendingEvents`:
  1. Pre-filter with the existing predicate: `const kept = events.filter((event) => !shouldDropAlreadyAppliedEvent(entry, eventSequence(event)));`
  2. Define `finalize = () => { if (isCurrentAttach(sessionId, entry, generation)) beginAttachSettle(sessionId, entry, generation, snapshot); };` (wrapped per 4.2: it cannot throw into xterm's parse loop).
  3. If `kept.length === 0`, call `finalize()` and return (no bytes to fence).
  4. Otherwise write each kept event in order via `writeTerminalBytes(entry, new Uint8Array(event.data), isLast ? finalize : undefined)` and `markAppliedSequence(entry, eventSequence(event))`; only the last kept write carries `finalize`. xterm's FIFO queue guarantees that `finalize` fires after every kept byte was parsed. **Every fenced replay write registers in the shared drain (4.1), unconditionally  -  the reconcile window is already closed here, so the registration must not reuse the `snapshotReplayPending` gate of 4.2** (Grinch finding 2): each replay write applies the same 0→1 promise replacement and increments `inFlight`; its completion callback first releases the drain (decrement, resolve at zero) and, for the last kept write, then runs `finalize` inside the same throw-free callback; a synchronous throw from a replay write releases the drain and stops the generation with the 4.2 warn. This makes a later generation's fence await an older generation's still-queued replay bytes before its reset  -  without it, a fast re-attach could reset while the older replay bytes are still in the FIFO, and those bytes would parse into the fresh buffer ahead of the newer snapshot and duplicate it (the issue's own straddle class).
- The filter uses the same `shouldDropAlreadyAppliedEvent` predicate as live arrivals, so there is no divergent second source of truth; events with `sequence <= snapshot.sequence` are inside the snapshot and are not replayed; events above it are replayed exactly once.

### 4.5 One-shot settle

- New `beginAttachSettle(sessionId, entry, generation, snapshot)`: `requestAnimationFrame(() => { if (!isCurrentAttach(sessionId, entry, generation)) return; void settleAttachViewport(sessionId, entry, generation, snapshot); });`
- New `settleAttachViewport(sessionId, entry, generation, snapshot)`:
  1. `entry.fitAddon.fit();`
  2. `const resizeOutcome = await sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);`
  3. `if (!isCurrentAttach(sessionId, entry, generation)) return;`
  4. `const drain = attachDrains.get(entry); if (drain && !drain.userScrolled) { entry.terminal.scrollToBottom(); }` (at most once per generation; the only `scrollToBottom` call site in the frontend; the guard is 4.5)
  5. Read `const buffer = entry.terminal.buffer.active;` and record the bounded, content-free settled instrumentation, one line per generation, covering every evidence item the issue's in-scope list names ("viewportY/baseY, terminal-grid, snapshot-grid, seed-size, history, and alternate-screen evidence at attach settle"): `console.debug("[terminal] attach " + sessionId + " settled: viewportY=" + buffer.viewportY + " baseY=" + buffer.baseY + " bufferLength=" + buffer.length + " cols=" + entry.terminal.cols + " rows=" + entry.terminal.rows + " type=" + buffer.type + " snapshotCols=" + String(snapshot.cols) + " snapshotRows=" + String(snapshot.rows) + " seedBytes=" + snapshot.data.length + " resize=" + resizeOutcome);` (the draft's record omitted the terminal grid, snapshot grid, seed size, and alternate-screen evidence; `type` is `'normal' | 'alternate'` per `IBuffer`, `snapshot.cols`/`snapshot.rows` may be `null`, and every value is a number or that enum  -  no content).
- `scrollToBottom` runs once regardless of the resize outcome (`"sent"`, `"deduplicated"`, or `"failed"`), unless the user-scroll guard (below) applies: the fit already fixed the xterm grid, the dedup case means the PTY already holds that grid, and the failure case is already handled by the existing `scheduleResizeRetry`; the viewport settle is about the xterm view, not the PTY grid. A resize failure still produces the settle record with `resize="failed"`.
- **User-scroll guard**: `createSessionTerminal` registers a passive `wheel` listener on `terminal.element` whose handler does `const entry = registry.get(sessionId); const drain = entry && attachDrains.get(entry); if (drain) drain.userScrolled = true;` (the handler resolves the entry by session id because the entry object does not exist yet at registration time). `beginSeed` clears the marker (4.1). When the marker is set, the settle skips `scrollToBottom` (step 4 above) while still recording the truthful metrics, so an intentional user scroll in the post-replay/pre-settle window (one rAF plus one resize round-trip) is never overridden by the one-shot settlement. The listener dies with the element on teardown. Residual (documented, not covered): touch, scrollbar-drag, and keyboard-driven scrolls in that sub-frame window do not emit `wheel`; the dominant desktop surface (mouse and trackpad, including over the scrollbar) emits `wheel`, and a `scroll` listener cannot be substituted because output-driven scrolling also fires `scroll` events, which would spuriously suppress the bottoming and break the "viewportY == baseY without user input" acceptance. The wheel-marker path is exercised by test 13's settle-window sub-step (10.2), which dispatches a real `wheel` event on `terminal.element` (Grinch finding 3); without it, no test sets `userScrolled` and the guard's skip branch would be unverified.
- `writeTerminalBytes` and `writeLivePtyOutput` never call `scrollToBottom`; the settle sequence is the only bottoming path. An intentional user scroll-up during ordinary live output, and (via the wheel guard) during the attach settle window, is therefore preserved.

### 4.6 Awaitable typed resize

`sendPtyResize` (319-347) changes from `void` to `Promise<"sent" | "deduplicated" | "failed">`:

- dedup hit: `return Promise.resolve("deduplicated");`
- sent path: return the `PtyAPI.resize(...)` promise mapped to `"sent"` on success (existing `resizeRetryAttempts = 0` reset kept) and to `"failed"` in the catch (existing `lastSentViewport` revert, warn, and `scheduleResizeRetry` kept).
- The three ordinary call sites become `void sendPtyResize(...)`: `syncViewport` (349-359), `scheduleResizeRetry` (292-317), and the `onResize` handler in `createSessionTerminal`. Only the settle awaits it.

### 4.7 Generation guard

New `isCurrentAttach(sessionId, entry, generation): boolean`:

```
registry.get(sessionId) === entry &&
!entry.destroyed &&
attachGenerations.get(entry) === generation &&
desiredSessionId === sessionId &&
attachedSessionId === sessionId &&
visibleSessionId === sessionId
```

It is checked by every async continuation of the attach transaction: each drain-loop iteration, the snapshot write callback, the flush finalize, the settle animation frame, and the resize continuation. A switch away, a superseding attach, a detach, an unmount, or an entry disposal flips at least one clause, so late continuations become inert. `attachedSessionId` is set to `null` by the chain before the detach invoke, which invalidates in-flight settles during a switch; `beginSeed` bumps the generation at every attach, which invalidates callbacks from older attachments of the same session. Aborted drains leave the reconcile window open only until the next `beginSeed` or an explicit close on the paths in 4.3; orphaned drain objects are garbage with their entry.

**Load-bearing invariants (verified during dev enrichment):** (a) Capturing the generation at `applySnapshot` time is equivalent to capturing it at `beginSeed` time: `transitionAttachment` serializes every continuation on `attachChain`, and `applySnapshot` runs synchronously inside the seed's own continuation, so no later `beginSeed` (generation bump) can execute before the capture. (b) The replay pipeline is fire-and-forget (`void replaySnapshotFenced(...)`), so the drain never blocks the chain: a switch issued during a drain proceeds immediately, and the drain loop's own `isCurrentAttach` check aborts on the first interleaving (`desiredSessionId`/`visibleSessionId` change, `attachedSessionId = null` before the detach invoke, or a generation bump from a re-attach). An abort leaves the shared drain's count intact, so the next generation still fences the older generation's unparsed writes before its reset (4.1)  -  the FIFO-residue case. (c) The snapshot settle timer is always cleared before any drain runs (`settleSeed` calls `concludeSnapshotFetch`, which clears it, before `applySnapshot`), so an aborted drain cannot produce a spurious `SNAPSHOT_SETTLE_WARN_MS` warning. (d) After an abort the reconcile window stays open but is inert and bounded: retention growth is capped by `SNAPSHOT_RECONCILE_LIMIT_BYTES` via `abandonSnapshotReconcile`, and the next `beginSeed` replaces the window wholesale and resets the drain's `userScrolled` marker (the drain itself is reused, 4.1).

### 4.8 Harness-fake compatibility contract (the proven blocker correction)

The frozen production mechanism exposes exactly two observable xterm-contract requirements that every existing fake in the repository must satisfy for the full suite to stay green:

- **The optional parse-completion callback** (`write(data, callback?)`): `writeLivePtyOutput`'s drain registration passes an `onWritten` and `drainPendingWrites` awaits it; a fake that never invokes the callback wedges the drain (render-gate, 2.6).
- **The settle's buffer read** (`buffer.active` with `viewportY`, `baseY`, `length`, `type` at production line 819): `settleAttachViewport` reads it unconditionally once the replay completes; a fake without `buffer` throws a `TypeError` inside the fire-and-forget settle (App.workflow, 2.6). Both fakes already have `scrollToBottom()` and `reset()` members, which the settle and the replay also call; neither member changes in either file: `scrollToBottom()` is a no-op in both, while `reset()` in both increments `resets` and clears `screen` (render-gate's tests assert `resets`).

Two existing fakes need correction, and exactly two (2.6):

1. **`TerminalView.render-gate.test.tsx`**: the mocked `Terminal.write(data: unknown)` becomes `write(data: unknown, callback?: () => void)` and invokes `callback?.()` when the write completes. The fake applies writes to `screen` synchronously, so firing the callback synchronously after `screen.push` is faithful to this file's model and to the App.workflow fake's existing behavior; it is also exactly what makes the drain resolve: the 0->1 promise replacement and the synchronous decrement keep `inFlight` at 0 between writes, so the drain loop exits on the first check and reset+replay run inside the same microtask turn the existing `waitFor`/`flushPromises` assertions expect. The fake additionally exposes `buffer: { active: { viewportY: 0, baseY: 0, length: 0, type: "normal" as const } }` (mutable object), which makes the settle record safe to compute; no render-gate test asserts any buffer metric or the settle record, so the constant zeros are behavior-neutral. `reset()` keeps clearing `screen` and keeps pending callbacks (xterm 6.0.0 does not discard the queue); with synchronous callbacks there are none pending in this file. Every existing assertion in its 8 tests is preserved exactly.
2. **`App.workflow.test.tsx`**: the fake already fires the optional callback (current line 97-102), so only the buffer read needs satisfying: expose the same `buffer: { active: { viewportY: 0, baseY: 0, length: 0, type: "normal" as const } }` mutable object. `write`, `reset`, `scrollToBottom`, and the callback behavior stay byte-for-byte as today. Every existing assertion in its 30 tests is preserved exactly.

The compatibility contract is minimal by construction: it adds only the members the frozen mechanism reads (the callback parameter plus the four buffer scalars and the `type` enum), mirrors section 5.2's fake semantics, and adds no test, helper, dependency, production fallback, or deadline. Residual acceptance: after both corrections the full suite must be **1601 green with zero unhandled errors**, with the attachment file at 17 tests, render-gate at 8, and App.workflow at 30, no assertion changed in any of them.

## 5. Exact files and symbols

### 5.1 `src/terminal/components/TerminalView.tsx`

| Symbol (current line) | Change |
|---|---|
| `AttachDrain` interface (new, 671-677, component scope, beside `SnapshotSettle` at 658-670) | Add |
| `attachGenerations`, `attachDrains` (new, 678-679, component scope) | Add |
| `writeTerminalBytes` (131-140) | Add optional `onWritten?: () => void`, forwarded to `terminal.write` |
| `beginSeed` (899-941) | Add generation bump and drain create-if-absent reset (4.1) |
| `writeLivePtyOutput` (509-564) | Keep signature; internal drain registration on the write path only while the reconcile window is open, with a throw-safe write (4.2) |
| `drainPendingWrites` (new, 576-593) | Add |
| `closeSnapshotReconcile` (new, 565-575) | Add (window close only; the drain is not deleted, 4.2) |
| `concludeSnapshotFetch` (681-701) | Stop clearing pending/retention/bytes; keep validation, timer clear, settle build |
| `failSeed` (956-982) | Add `closeSnapshotReconcile(entry)` |
| `applySnapshot` (830-898) | Seedless and discard branches close the window; snapshot path captures generation and calls `replaySnapshotFenced`; remove `scheduleViewportSync` from the snapshot path only |
| `replaySnapshotFenced` (new, 728-776; replaces `rebuildFromSnapshot` 557-568) | Add |
| `flushPendingEventsFenced` (new, 594-638; replaces `flushPendingEvents` 493-500; takes `snapshot` for the settle evidence) | Add |
| `beginAttachSettle` (new, 777-796; takes `snapshot`) | Add |
| `settleAttachViewport` (new, 797-829; takes `snapshot`; `buffer.active` read at 819) | Add |
| `isCurrentAttach` (new, 639-650) | Add |
| `sendPtyResize` (349-379) | Return `Promise<"sent" \| "deduplicated" \| "failed">`; keep dedup, drift report, retry, revert |
| `syncViewport` (380-391), `scheduleResizeRetry` (318-348), `onResize` handler in `createSessionTerminal` (141-296) | Prefix the `sendPtyResize` call with `void` |
| `createSessionTerminal` (141-296) | Register the passive `wheel` user-scroll marker listener on `terminal.element` (4.5; the listener is at 261-268) |

No imports are added, removed, or changed. No constant changes. `transitionAttachment`, `settleSeed`, `scheduleViewportSync` (as called by `selectSession`, `ResizeObserver`, and the seedless path), `resizeTerminalForSnapshot`, `handlePtyOutput`, `onMount`, `createEffect`, `onCleanup`, and the JSX are unchanged.

### 5.2 `src/terminal/components/TerminalView.attachment.test.tsx`

| Symbol (current line) | Change |
|---|---|
| `FakeTerminalInstance` (36-55) | Add `buffer: { active: { viewportY: number; baseY: number; length: number; type: "normal" | "alternate" } }`, `scrollToBottomCalls: number`, `pendingWriteCallbacks: Array<() => void>`, `writeThrows: boolean` (the mocked `Terminal` class `implements FakeTerminalInstance`, so the interface change forces the class fields; `type` is the settle's alternate-screen evidence; `writeThrows` models xterm's 50-MiB queue guard) |
| Mocked `Terminal` class (`vi.mock("@xterm/xterm")`) | Queue-semantics `write(data, callback?)`: record bytes in `writes` immediately; if `writeThrows` is set, throw synchronously before queueing (xterm's 50-MiB guard); otherwise apply to `screen` and fire the callback only when the write completes (`queueMicrotask` when `xterm.autoCompleteWrites` is true, otherwise into `pendingWriteCallbacks`). `reset()`: increment `resets`, clear `screen`, zero `viewportY`/`baseY`/`length`, leave `type` untouched (tests set it explicitly), and keep pending callbacks (xterm 6.0.0 does not discard the queue). `scrollToBottom()`: increment `scrollToBottomCalls`, set `viewportY = baseY`. `resize()` unchanged |
| Hoisted `xterm` object | Add `autoCompleteWrites: true`; `beforeEach` resets it to `true`, clears instance callback queues, and resets every instance's `writeThrows` to `false` |
| New helpers | `completeWriteCallbacks(instance)` (drains `pendingWriteCallbacks` FIFO), `simulateParsedHistory(instance, lines)` (sets `length = lines`, `baseY = lines - rows`, `viewportY = 0`), `simulateUserScrollUp(instance)` (sets `viewportY = 0`) |
| New fixtures | `MULTIVIEW` (five viewports of 24 rows of 80-column line bytes), `RING_64K` (64 * 1024 bytes), named Codex and Pi lifecycle shapes built from these plus overlapping live events |
| New tests | 8 tests, named in section 10.2 |

The existing 9 tests keep their assertions exactly; the only mechanical change is the `beforeEach` reset (auto-complete on, callback queues cleared, `writeThrows` cleared). Verified per test: 1 (chain supersede; no screen assertions), 2 (rejected attach; `writes`/`instancesFor` only), 3 (listener scope), 4 (listener gate; `screen` asserted inside `waitFor`), 5 (fail-closed), 6 (unsequenced exactly once; `writes` and `resets` only), 7 (re-seed `screen`/`resets`/attach ids), 8 (two views), 9 (resync; exactly one `pty_resize` `{80,24}` per attach interval survives because the settle's resize is deduplicated by the priming sync, and the seedless re-attach keeps `scheduleViewportSync`).

### 5.3 `src/terminal/components/TerminalView.render-gate.test.tsx` (compatibility only)

| Symbol (current line) | Change |
|---|---|
| `FakeTerminalInstance` (38-49) | Add `buffer: { active: { viewportY: number; baseY: number; length: number; type: "normal" | "alternate" } }` |
| Mocked `Terminal.write` (89-93) | Accept optional `callback?: () => void`; keep recording bytes in `writes`/`screen` synchronously; invoke `callback?.()` when the write completes (synchronous, faithful to this file's model) |
| Mocked `Terminal` class fields | Add `buffer` initialized to `{ active: { viewportY: 0, baseY: 0, length: 0, type: "normal" } }` (mutable object; no render-gate test reads or asserts it) |
| `reset()` (95-98), `scrollToBottom()` (100), `writeThrows`-free write path | Unchanged (this file does not need the queue gate; it never gates writes) |

The interface change forces the class field through `implements FakeTerminalInstance`. All 8 tests keep their names and assertions exactly; no `beforeEach`/`afterEach` change is needed in this file (its setup already resets `xterm.instances`, and the new members are constant).

### 5.4 `src/terminal/App.workflow.test.tsx` (compatibility only)

| Symbol (current line) | Change |
|---|---|
| `FakeTerminalInstance` (39-53) | Add `buffer: { active: { viewportY: number; baseY: number; length: number; type: "normal" | "alternate" } }` |
| Mocked `Terminal` class fields | Add `buffer` initialized to `{ active: { viewportY: 0, baseY: 0, length: 0, type: "normal" } }` (mutable object; no workflow test reads or asserts it) |
| `write(data, callback?)` (97-103) | **Unchanged**: keeps firing `callback?.()` on completion (its #1283 admission settles replay/write gates from it); the drain and settle now complete instead of throwing |
| `reset()` (106-109), `scrollToBottom()` (111) | Unchanged |

All 30 tests keep their names and assertions exactly. No `beforeEach`/`afterEach` change is needed: the suite's existing per-test instance reset already covers the new constant member.

## 6. Behavior, edge cases, and failure handling

1. **Snapshot with retained live events**: retained events were written live on arrival and queued before the reset; the drain waits until every such write parsed, then reset+snapshot run, then the watermark-filtered replay writes them exactly once after the snapshot. Final screen: snapshot bytes, then live bytes, exactly once. Residual nuance (do not "fix"): a live write arriving after `closeSnapshotReconcile` but before the snapshot bytes finish parsing is written live, is not retained, and therefore parses between the snapshot bytes and the retained replay bytes. Every byte still appears exactly once; the retained replay can sit after a small number of post-close live bytes because retained bytes can only be replayed once the snapshot is parsed.
2. **No retained events**: the drain is empty, the snapshot write callback triggers the settle directly.
3. **All retained events inside the snapshot** (`sequence <= snapshot.sequence`): the pre-filter keeps none; `finalize` runs immediately after the snapshot callback; no duplicate bytes.
4. **Reconcile budget overrun during the drain**: `abandonSnapshotReconcile` clears the window; the post-drain re-check discards the snapshot with the existing warning; no reset, no replay, live content stands (identical semantics to today's apply-time discard, detected later by at most one drain).
5. **Switch during attach (A -> B -> A)**: the chain detaches and re-attaches; `beginSeed` bumps the generation; every continuation of the older generation fails `isCurrentAttach` and is inert; only the newest generation drains, replays, and settles once. The replay pipeline is fire-and-forget, so the chain is never blocked by a drain: a switch during a drain aborts it at the next `isCurrentAttach` check, and the chain's serialization guarantees the newer `beginSeed` runs only after the older continuation has finished, so a stale `applySnapshot` can never capture the newer generation (see the invariants in 4.7). The older generation's still-queued xterm writes are **not** discarded: the window-registered live writes (4.2), the snapshot write (4.4 step 6), and the fenced replay writes (4.4 step 4) stay counted in the shared drain, so the newest generation's fence awaits them before its reset, and FIFO order puts them (and any post-close live bytes riding behind them) ahead of the newest snapshot bytes  -  no stale byte can land inside a newer replay. This holds only with the 4.4 snapshot-write and replay registrations: an unregistered snapshot or replay write would escape the fence and could parse into the newer buffer after its reset (Grinch finding 2). Boundary residual (adjudicated at the Step 7 consensus, round 2; see 4.1): a post-close live byte queued after the newest counted write and immediately before a sub-12-ms switch is a documented out-of-scope, pre-existing residual and does not contradict this case's guarantee; do not fix.
6. **Dispose during attach**: `registry.get(sessionId) !== entry` or `entry.destroyed` stops the drain loop, the write callback, the flush, and the settle frame; no terminal mutation after disposal.
7. **Write callback never arrives**: verified against xterm 6.0.0's `_innerWrite`: the loop time-slices at ~12 ms and advances across per-chunk parse rejections (xterm rethrows the rejection as an unhandled microtask error and continues), and terminal `dispose()` does not cancel the loop's scheduled continuation, so queued chunks still complete and the drain resolves; the only synchronous stall is `write()`'s own 50-MiB queue guard throw, which the throw-safe write path releases (4.2) before rethrowing, so `inFlight` can never strand and no generation can wedge. No new deadline machinery is added.
8. **Resize rejects**: `sendPtyResize` resolves `"failed"`, the existing retry is scheduled, and the settle still bottoms exactly once and records `resize="failed"`.
9. **Resize deduplicated**: the PTY already holds the fitted grid; the settle proceeds immediately with `resize="deduplicated"`.
10. **User scrolled up before a live write**: live writes never call `scrollToBottom`; the viewport stays where the user put it. **User scrolled during the settle window** (after the replay, before the settle): the wheel marker (4.5) suppresses the settle's `scrollToBottom`, so the one-shot settlement never overrides an intentional scroll; without the marker the settle bottoms exactly once.
11. **Attach while the terminal is hidden** (switch-away in flight): `visibleSessionId` clause fails; the settle does not run for the hidden session.
12. **Unsequenced events** (parser unavailable): watermark never drops them (sequence null), the drain still counts them, the replay writes them once after the snapshot; the seedless path (no snapshot) keeps today's write-once, no-reset behavior.
13. **Near-64-KiB ring replay**: one 64-KiB batch plus overlapping live events drain, replay exactly once, and settle once; the frontend never truncates (the ring bound is backend-owned).
14. **Empty snapshot components**: a zero-length `snapshot.data` follows the seedless branch (no reset, no replay); an all-dropped retention follows case 3.
15. **A fake that never fires the write callback** (the render-gate wedge): the drain awaits forever and reset+replay never run. This is the proven blocker; the compatibility contract (4.8) removes it by construction: render-gate's `write` invokes the optional callback on completion, so `inFlight` returns to 0 and the drain resolves. A future fake that again omits the callback is a full-suite gate failure, not a production concern: `drainPendingWrites` awaits only the callbacks this plan's own write path registers.
16. **A fake without the settle's buffer read** (the App.workflow crash): `settleAttachViewport` throws `TypeError: Cannot read properties of undefined (reading 'active')` inside the fire-and-forget settle, surfacing as unhandled errors while assertions pass. The compatibility contract (4.8) removes it by construction: both fakes expose `buffer.active` with the four scalars and `type`. The production settle keeps reading `buffer` unconditionally (the read is the mandated instrumentation, section 4.5); the fakes, not the product, adapt.

## 7. Compatibility, performance, privacy, and security

### Compatibility

- Preserves the PTY flow (input -> `pty_write`; `pty_output` -> xterm) and the #961 seed contract (live bytes never gated behind the seed).
- Preserves the #1355 single-write-site invariant: `terminal.write` still has exactly one call site, now with an optional callback.
- Preserves the attach/detach chain serialization and desired-state checks (#1363) and the viewport dedup key invalidation per attach (#1439).
- No IPC, schema, type, backend, config, or dependency change; `PtyScreenSnapshot`, `PtyOutputEvent`, and `SessionTerminalEntry` are untouched.
- The harness queue semantics match xterm 6.0.0's actual FIFO behavior, so the tests are deterministic in jsdom and truthful about the production dependency.
- The two compatibility corrections (4.8) add only members the frozen production mechanism reads: the optional write callback and the `buffer.active` metrics. No import, no assertion, no test count, and no product behavior changes in `TerminalView.render-gate.test.tsx` (8 tests) or `App.workflow.test.tsx` (30 tests).

### Performance

- The drain adds one callback per live write only while an attach is pending (already the retention window); no per-paint work.
- The settle adds one fit, one dedup-aware resize, one bottom, and one debug line per attach generation.
- No timers added; the existing `SNAPSHOT_SETTLE_WARN_MS` warn timer is unchanged.

### Privacy and security

- The settled instrumentation records only numeric viewport metrics, the terminal/snapshot grid, the seed byte count, the buffer type enum (`normal`/`alternate`), and the resize outcome; no bytes, content, prompts, commands, or user data. It is `console.debug` (existing log surface of this file), bounded to one line per generation.
- No new permissions, transports, or persisted data.

## 8. Implementation order

The branch may be red while reproducing evidence locally, but no intermediate red PR or merge may land; the production fix, the harness extension, and the two compatibility corrections land together in one green PR. Steps 1-4 were already executed during the Step 8 cold implementation and are preserved uncommitted in the working tree (section 2.1); a cold implementer verifies them (focused 17 green, typecheck, dependency gate, build) and does not redo or alter them. Steps 5-7 are the remaining work this revision adds.

1. **Harness extension only** (`TerminalView.attachment.test.tsx`): queue-semantics fake, buffer metrics, `scrollToBottom` counter, gated completion control, helpers, fixtures, `beforeEach` reset. Run the focused file: the 9 existing tests must stay green (they prove the extension is behavior-preserving for the old assertions). [Executed and preserved: 17/17 green on the final preserved head.]
2. **Deterministic pre-fix evidence** (against the untouched production file): the two evidence tests from section 10.2 (replay-ordering, attach-settle position) fail deterministically and are recorded with actual-vs-expected output (test 10: duplicated/straddled replay; test 11: `scrollToBottomCalls == 0` and `viewportY < baseY` after attach settle). Record the evidence in the final issue comment of #1489, the durable location resolved in section 9.2, and repeat it in the PR body at merge; never merge red. [Executed and preserved: evidence verified at `scratch/1489-step8/pre-fix-red-evidence.txt`; matches the section 2.4 predictions exactly.]
3. **Production fix** (`TerminalView.tsx`): implement sections 4.2-4.7 (drain, close helper, fenced replay, settle, generation guard, awaitable resize). `npm run typecheck` must pass. [Executed and preserved: +326/-39, audited faithful in the Step 8 blocker report; the Step 6 Grinch enrichment and the architect Step 7 adjudication add three required deltas to the preserved file - the throw-safe snapshot write, the snapshot-write drain registration, and the fenced-replay drain registration (4.2/4.4, findings 1-2) - which the implementer applies here and re-verifies with the focused file and the full-suite gate.]
4. **Behavior tests green**: the remaining six tests are active; the focused file runs 17/17 green. [Executed and preserved.]
5. **Harness-fake compatibility corrections** (the proven blocker fix, section 4.8): `TerminalView.render-gate.test.tsx` (write accepts/invokes the optional callback; `buffer` metrics) and `App.workflow.test.tsx` (`buffer` metrics only). Run the three focused files: attachment 17 green, render-gate 8 green, App.workflow 30 green, no assertion changed.
6. **Full verification**: the three focused files, `npm test` (must be **1601 green, zero unhandled errors**), `npm run typecheck`, `npm run check:frontend-dependencies`, `npm run build`, and the Step-N cycle gate (section 11) all green on the final head.
7. **Land** exactly one green PR containing only the four files; no other path may change. The final conventional commit references #1489 and contains exactly six paths: the four TS/TSX files plus the two byte-identical plan artifacts force-added (section 9.4).

## 9. Resolved operational decisions

The two operational questions raised in the Step 4 draft were resolved against repository evidence. No open choice, TBD, competing alternative, or implementer decision remains.

### 9.1 Canonical verification command set (resolved)

The minimal canonical command set recorded for the implementer is exactly the seven commands in section 10.3 (three focused files: attachment, render-gate, App.workflow; full suite; typecheck; frontend-dependencies gate; build), run from the repository root in the PR workflow environment (Node 22, npm 11.6.2, after `npm ci`). The two extra focused runs are required because the compatibility corrections make two more test files in-scope: their per-file green result is the proof that no assertion changed. Evidence:

- `.github/workflows/pr-regression-gates.yml` runs `npm run typecheck` and `npm test` (with the CI-owned #480 known-debt guard) in the `frontend-regression` job, and runs `npm run build` as frontend asset validation for Tauri config in the `rust-regression` jobs.
- The canonical #1478 plan (`plans/1478-terminal-attach-replay.md`, section 10) records the same command set for this exact test file: focused attachment command, full suite, typecheck, `check:frontend-dependencies`, and build.
- `package.json` has no lint script and no ESLint dependency: no lint command exists and none is invented, and no new lint suppressions may be added. The husky pre-push hook validates only branch naming (`scripts/validate-branch-name.mjs`, CONTRIBUTING.md); it is not a code-quality gate.
- No Rust command applies: this plan changes no `src-tauri` file.
- The Step-N detector command set of section 11.2 is part of the recorded set and stays exactly as written there.

### 9.2 Pre-fix evidence location (resolved)

The deterministic pre-fix evidence for tests 10 and 11 is recorded in the **final issue comment of #1489**, which is the durable record mandated by the #1478 epic completion criteria ("#1489 links its approved plan/HTML digests, deterministic pre-fix evidence, PR, merge SHA, focused and full verification evidence, and deployed build from its final issue comment"), and is repeated in the PR body at merge time for reviewer visibility. No repository file may be created for the evidence.

### 9.3 Certification record (architect Step 7 consensus, round 2, complete)

The prior certification (`READY_FOR_IMPLEMENTATION`, plan `0DB8084612431756D84CD153233FC57DEC7BAD697B6A94F952469BB7D75AC75A`, HTML `39DC567CB95A5754FA3E7AA7D63DD445C80A01412D906064780C28AD1780CF1D`, clean-base Step-N record in `scratch/1489-step7/`) was **invalidated** by the Step 8 implementation evidence in section 2.6: the exact two-file scope proved insufficient, exactly as the plan's own blocker clause (§3) commanded. This plan restarted as a Step 4 draft with the proven four-file scope. Round 1 of the Step 7 consensus returned `NEEDS_ANOTHER_ROUND` (architect message `20260822-070237`): Grinch findings 1 (throw-safe snapshot write) and 3 (wheel-guard coverage) were adjudicated correct and complete; finding 2 (shared-drain registration) was adjudicated necessary but incomplete because the snapshot write itself (4.4 step 6) was a third write class left unregistered, and test 12's main body encoded the un-fixed ordering. Round 2 applied all seven corrections (dev report `20260822-070714`, `DEV_ENRICHMENT_COMPLETE`): the three-class drain invariant (4.1), the snapshot-write registration composing findings 1-2 (4.4 step 6), the 6.5 coverage sentence, the deterministic test-12 main-body re-sequencing (10.2), the three implementer deltas (8 step 3), the Grinch Review adjudication note, and the U+2014 normalization (final count zero). The Grinch round-2 review (report `20260822-071549`, `GRINCH_ENRICHMENT_COMPLETE`) verified the correction adversarially (release discipline exactly once with mutually exclusive release paths; release-then-flush ordering load-bearing; no deadlock, no double release, no stranding; test-12 main body traced end-to-end; no forbidden additions, scope, or path expansion), applied one consistency edit (section 12's "two production deltas" to "three"), and raised a residual observation that this consensus **adjudicates as a documented, out-of-scope, pre-existing residual** (recorded in 4.1 and 6.5): a post-close live byte queued after the newest counted write immediately before a sub-12-ms A→B→A switch can parse ahead of the newer snapshot with at most a few duplicated bytes already reflected in that snapshot; it is pre-existing (#961 live-on-arrival), orders of magnitude narrower than the straddle this plan fixes, unmodelable by the harness, fixable only by unconditional post-close registration that would re-open the section 2.6 fake-compat analysis and violate the four-file scope, and it does not contradict the issue's acceptance. The clean-base Step-N record (`scratch/1489-step7/pre.json` + `pre-arcs.txt`) is preserved and remains valid: no Rust change, frontend-only edits, detector exit 1 (pre-existing gating cycles reported, graph still written), regenerated arc record byte-identical, single cyclic SCC unchanged with identical member sets, frontend dependency gate PASS. The architect verdict is `READY_FOR_IMPLEMENTATION` (section 12).

Remaining non-user gates execute downstream of this certification and stay mandatory: exact plan and HTML hashes, plan/HTML validation and freeze, complete participant reports, the peer-idle check, the workgroup purge dry-run, digest recomputation, the real purge, cold implementation, review, current-main, landing, and build/deploy.

**Recorded user waiver (fixed decision, not an open choice):** the user explicitly waived the Step 7.5 approval pause for this issue  -  their approval of the impact HTML is NOT required, and no pause for or Browser/raised-hand approval wait is performed; implementation proceeds on the remaining gates above. The waiver removes no non-user gate: architect consensus/`READY_FOR_IMPLEMENTATION`, exact plan and HTML hashes, plan/HTML validation and freeze, digest recomputation, purge, cold implementation, review, landing, and build/deploy all remain mandatory. The waiver survives this restart unchanged; it is a recorded fact, not a question.

### 9.4 Final commit expectations (resolved)

- The final implementation file set is exactly four existing files: `src/terminal/components/TerminalView.tsx`, `src/terminal/components/TerminalView.attachment.test.tsx`, `src/terminal/components/TerminalView.render-gate.test.tsx`, `src/terminal/App.workflow.test.tsx`. #1489 owns all four exclusively; no later child issue may reopen any of them for this defect.
- The final conventional commit references #1489 and contains exactly **six paths**: the four TS/TSX files above plus the two byte-identical plan artifacts (`plans/1489-terminal-attach-replay-settlement.md`, `plans/1489-terminal-attach-replay-settlement-impact.html`), force-added as delivery evidence with both hashes preserved. No other path may appear in the commit; no commit exists until every gate in section 8 step 6 is green on the final head.

## 10. Focused tests and objective acceptance

### 10.1 Objective acceptance (from the issue, mapped)

- Separate deterministic pre-fix evidence records replay-ordering failure (test 10 red: retained live bytes precede and duplicate the snapshot) and attach-settle failure (test 11 red: `viewportY < baseY` after attach settle).
- Final focused coverage proves the terminal output equals snapshot bytes followed by overlapping live bytes exactly once (tests 10, 15, 16, 17).
- Attach, detach/reattach, and switch-away/switch-back cannot leave an older async write able to mutate the current replay (tests 12, 16, 17; existing tests 1, 7, 9).
- After replay and fit settle, instrumentation records `viewportY == baseY` without user input (tests 11, 14, 15; settled `console.debug` assertion).
- Attach-settle evidence also covers the terminal grid, snapshot grid, seed size, history depth, and alternate-screen state in the single settled `console.debug` line (record fields in 4.5; `type == "normal"` asserted in tests 11 and 14, `type == "alternate"` asserted in test 16).
- Deliberately scrolling to the top shows meaningful reconstructed history, not a synthetic empty region (test 14: `bufferLength == 120` and non-empty `screen`).
- A user intentionally scrolled up remains scrolled up while ordinary live output arrives (test 13) and, via the settle's wheel guard (4.5), through the attach settle window (test 13's settle-window sub-step dispatches a real `wheel` event on `terminal.element` and asserts the one-shot settlement does not bottom).
- Evidence covers Codex and Pi lifecycles (tests 16, 17), more than one viewport (tests 11, 14, 16), and ring history at or near 64 KiB (tests 15, 17).
- Focused attachment tests and the normal repository test path are green at merge (section 8 step 6): the three focused files run 17 + 8 + 30 green and the full suite runs **1601 green with zero unhandled errors**, with no assertion changed in any of the three test files (the two compatibility corrections prove this by per-file green, section 4.8).
- Only the four exclusively owned files change (`TerminalView.tsx`, `TerminalView.attachment.test.tsx`, `TerminalView.render-gate.test.tsx`, `App.workflow.test.tsx`); the final commit contains exactly those four plus the two byte-identical plan artifacts (section 9.4).

### 10.2 Test list (final file: 17 tests)

Existing 9 (names and assertions unchanged):

1. never attaches a superseded target and ends attached to the last selection
2. keeps transitioning after a rejected attach, and owes no detach for it
3. registers the pty_output listener scoped to this window
4. does not attach until the pty_output listener has registered
5. leaves the window unattached when the listener registration fails
6. writes an unsequenced chunk exactly once when the attach returns no snapshot
7. shows the output produced while detached, with no gap and no duplicated block
8. gives each mounted view its own attachment and writes only the visible session
9. a re-attach that resolves without a snapshot resyncs the viewport before live writes land

New 8:

10. **waits for the snapshot write to parse before replaying retained live output** (pre-fix evidence, replay ordering). Gated writes; the attach's snapshot carries `sequence: 1` (pinned below `LIVE`'s 5: with `sequence >= 5` the replay watermark would drop `LIVE` and the pre-fix red assertion would not occur); emit `LIVE` (sequence 5) while the attach is pending; resolve the attach; assert `writes == [LIVE]` and `screen == []` while the drain holds (the live write is queued, not applied); complete the live callback (the drain resolves; the reset runs; the snapshot write is queued); assert `writes == [LIVE, SNAP]` before completing the snapshot callback; complete the snapshot callback (the replay write is queued); complete the replay callback; assert `screen == [SNAP, LIVE]` exactly once. Pre-fix this is red: `screen == [LIVE, SNAP, LIVE]`.
11. **settles the viewport to the current screen exactly once after replay and fit** (pre-fix evidence, attach-settle position). Snapshot carries `rows: 27, cols: 81` so the settle's resize is not deduplicated; gate `pty_resize` with a deferred handler; wait for the settle's `pty_resize` invoke to fire (proof the replay and the reset ran  -  the reset zeroes the buffer metrics, so the history simulation must come after it); then `simulateParsedHistory(instance, 135)`; assert `scrollToBottomCalls == 0` (no bottom before the resize outcome); resolve the resize; wait for `scrollToBottomCalls == 1`; assert `viewportY == baseY == 108`; assert the settled `console.debug` records the full evidence set: `viewportY`, `baseY`, `bufferLength == 135`, `cols == 81`, `rows == 27`, `type == "normal"`, `snapshotCols == 81`, `snapshotRows == 27`, `seedBytes` (the fixture's byte count), and `resize == "sent"`. Pre-fix this is red: `scrollToBottomCalls == 0` and `viewportY (0) < baseY (108)`.
12. **a stale attach generation cannot mutate a newer replay**. Pin the sequence so both generations exist: attach A (generation 1, snapshot write gated); switch to B (auto-complete, settles once); switch back to A (generation 2) - while generation 1's gated snapshot callback is still held, first assert generation 2's snapshot write is NOT queued (the corrected shared-drain fence holds it: the snapshot-class variant of sub-step A, proven before the release, not completed after the fact); then complete generation 1's held callback; assert generation 2's snapshot write now queues, generation 2 settles once, and generation 1's later continuation (its flush) is inert - no additional writes, no additional `scrollToBottom`, no additional `pty_resize`, and no settled record for generation 1. Variant sub-step A (FIFO residue, 4.1): while generation 1's fetch is pending and writes are gated, emit a live event for A (queued, not applied); switch to B and back to A; generation 2 resolves with no live events of its own; assert generation 2's snapshot write is NOT queued until generation 1's gated live write is completed (the shared drain fences it  -  without the fix the fresh drain would let the reset run while the stale byte is still queued); complete it; assert the final screen has no generation-1 byte before the generation-2 snapshot, exactly-once content, and one settle per generation. Variant sub-step B (throw release, 4.2): with writes gated, set `writeThrows = true`; `expect(() => fake.emitFromBackend("pty_output", { sessionId: A, data: LIVE })).toThrow()` (the transport calls listeners synchronously); set `writeThrows = false`; resolve the attach; assert the drain released (the snapshot write is queued) and the settle completes  -  `inFlight` must not strand. Variant sub-step C (unregistered replay residue, 4.4, Grinch finding 2): generation 1 resolves with a retained set and its flush queues the replay bytes (gated); before those bytes complete, switch to B and back to A; assert generation 2's snapshot write is NOT queued until generation 1's replay bytes complete (the fenced-replay registration makes the next generation's drain await them); complete them; assert no generation-1 byte before the generation-2 snapshot and exactly-once screen content.
13. **ordinary live output never bottoms a user who scrolled up**. After a settled attach, `simulateUserScrollUp`; emit `LIVE`; wait for the write; assert `scrollToBottomCalls` unchanged and `viewportY == 0`. Settle-window sub-step (wheel guard, 4.5, Grinch finding 3): switch away and back (second generation, gated writes); complete the second replay; before the settle's rAF fires, dispatch `new WheelEvent("wheel")` on `terminal.element`; assert `scrollToBottomCalls` stays at the first settle's count and the second settled record is still emitted (the marker suppresses only the bottoming, not the record).
14. **attach settle preserves meaningful multi-viewport history**. `MULTIVIEW` snapshot; `simulateParsedHistory(instance, 120)`; wait for settle; assert `viewportY == baseY == 96`, `bufferLength == 120`, and the settled record's `type == "normal"` with the snapshot grid and `seedBytes` matching the `MULTIVIEW` fixture; then scroll to the top and assert the reconstructed `screen` is non-empty.
15. **near-64-KiB ring replay stays exactly-once with one settle**. `RING_64K` snapshot (sequence 0) with two live events (sequences 5, 6) while pending; wait for `screen == [RING, LIVE5, LIVE6]`; assert `scrollToBottomCalls == 1` and `viewportY == baseY`.
16. **Codex lifecycle: idle multi-viewport replay, overlap, scroll-up, and re-attach**. Multi-viewport idle scrollback snapshot with overlapping live events; settle at bottom; scroll up; further live output stays put; switch away and back; exactly one settle per attach and no stale mutation. Alternate-screen evidence: before the second attach's settle, set `buffer.type = "alternate"`; assert the second settled record carries `type == "alternate"` and `viewportY == baseY` (the alternate buffer has no scrollback, so the settle's bottoming is a no-op there)  -  the issue's alternate-screen evidence is asserted, not merely named.
17. **Pi lifecycle: ring replay with switch-away/back and exactly-once live bytes**. Near-64-KiB ring with gated writes; switch away mid-replay and back; complete both generations in reverse order; assert exactly-once screen content, one settle per attach, and inert stale callbacks.

**Compatibility test files (no new tests, no assertion changes):** `TerminalView.render-gate.test.tsx` keeps its 8 existing tests (names and assertions exactly as today; only its fake gains the optional write callback and the `buffer` metrics, section 5.3) and `App.workflow.test.tsx` keeps its 30 existing tests (only its fake gains the `buffer` metrics, section 5.4). The per-file focused runs (10.3) are the proof that every pre-existing assertion still holds.

### 10.3 Verification commands

Resolved canonical set (section 9.1), run from the repository root with Node 22 and npm 11.6.2 after `npm ci`:

- Focused attachment: `npm test -- src/terminal/components/TerminalView.attachment.test.tsx --reporter=default` (final: 17 tests green; preserved Step 8 head already green).
- Focused render-gate (in-scope compatibility file): `npm test -- src/terminal/components/TerminalView.render-gate.test.tsx --reporter=default` (final: 8 tests green, assertions unchanged).
- Focused App.workflow (in-scope compatibility file): `npm test -- src/terminal/App.workflow.test.tsx --reporter=default` (final: 30 tests green, assertions unchanged).
- Full suite: `npm test -- --reporter=default` (the PR workflow's `npm test` runs with the CI-owned #480 known-debt guard and JSON reporters; the local CI-equivalent is this command). **Final: 1601 passed, zero errors. Blocker baseline recorded in section 2.6: 2 failed / 1599 passed (1601), 4 unhandled errors; the two compatibility corrections must close exactly that gap with no assertion change.**
- `npm run typecheck` (CI frontend-regression TypeScript gate).
- `npm run check:frontend-dependencies` (dependency-cruiser pinned from this repo's `node_modules`, over the complete `src` root; frontend dependency policy gate).
- `npm run build` (CI frontend asset build validation for Tauri config; recorded in the #1478 plan section 10 for this exact test file).
- No Rust commands apply: no `src-tauri` file changes.
- No lint command exists (`package.json` has no lint script and no ESLint dependency); none is invented and no new lint suppressions are added. The husky pre-push hook validates branch naming only (`scripts/validate-branch-name.mjs`) and is not a code-quality gate.
- The Step-N cycle gate commands stay exactly as recorded in section 11.2.

## 11. Dependency-cycle gate

### 11.1 Draft arc analysis

This plan adds **zero** module-pair dependency arcs on either side:

- `TerminalView.tsx`: no import is added, removed, or changed; all new symbols are local to the component or module scope of the same file. The `WeakMap` keys are existing `SessionTerminalEntry` objects already owned by this file's registry usage; no reference crosses a module boundary.
- `TerminalView.attachment.test.tsx`: no import changes; the new helpers and fixtures are file-local; it continues to use only the already-imported `FakeTransport`, ui-harness helpers, `SESSION_A`/`SESSION_B`, and `rememberSpawnViewport`.
- `TerminalView.render-gate.test.tsx` and `App.workflow.test.tsx`: compatibility-only changes (4.8, 5.3, 5.4) add an optional parameter, a callback invocation, and a `buffer` field; **no import is added, removed, or changed** in either file, so no module-pair arc exists on either side.
- Rust: no `src-tauri` file changes; the existing module structure and arc record are untouched.

No SCC can grow or join; no arc crosses a previously clean SCC boundary. This is a manual draft result only and does not certify the unimplemented change.

### 11.2 Required Step N detector acceptance

Immediately before the implementation freeze/purge gate and again on the final implementation, run the repository's Rust levelization detector from a clean tree against both the fixed base SHA and the final branch head, preserve both emitted graphs and the canonical arc record, and run the frontend dependency gate:

```
node "D:\0_repos\AgentsCommander_iac\.ac\wg-17-dev-v5-team\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet   (clean base checkout)
node "D:\0_repos\AgentsCommander_iac\.ac\wg-17-dev-v5-team\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet  (clean final head)
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
npm run check:frontend-dependencies
```

The detector path is the exact one resolved by the #1478 plan section 12.2 (the same draft placeholder was corrected there because an invalid detector path was recorded; the #1489 draft's `<vault>` placeholder is hereby resolved to it). Run from clean, separate worktrees for the fixed base and the final head, and store the emitted graphs and the regenerated arc record in the executing agent's allowed replica root (procedure per #1478 plan 12.3).

Green iff all five conditions hold:

1. `cyclicSccs` count is unchanged from base.
2. Every cyclic SCC has an identical member set before and after.
3. There are zero new arcs crossing a previously clean SCC boundary.
4. The canonical `src-tauri/module-arcs.txt` is byte-identical (empty `git status` on it).
5. Layering guards pass: no `tauri`/`AppHandle` dependency below commands (no Rust change at all), and the frontend helper logic stays in the UI/transport-owning module (`TerminalView.tsx`); the registry module keeps its `no-terminal-helper-back-edge` boundary untouched.

Detector exit semantics (verified against the detector source): exit 0 means no gating cycles (with a matching baseline: no NEW cycles); exit 1 means cycles were reported and still yields a graph; exit 2 is a usage error; exit 3 means the analysis was incomplete (error-level diagnostics) or the graph integrity check failed, and in both cases the graph is NOT written  -  exit 3 is a gate failure. Certification is green only if all five conditions hold.

## 12. Verdict

`READY_FOR_IMPLEMENTATION`

The plan is certified by the architect at the Step 7 consensus (round 2). Every decision is resolved and implementable within the exact four-file scope (one production file, three test files): the three-class shared-drain invariant (window-registered live writes, the snapshot write, the fenced replay writes) with the release-then-flush ordering and the exactly-once throw-safe release discipline closes the straddle/duplication defect class completely for every write this plan queues; the deterministic test-12 re-sequencing (main body as the snapshot-class fence variant, sub-steps A/B/C for live, throw, and replay residue) and the wheel-guard sub-step of test 13 keep the 17-test contract truthful; the two compatibility corrections (4.8) close the observed full-suite gap (blocker baseline: 2 failed / 1599 passed, 4 unhandled errors; required final: 1601 green, zero errors, no assertion changes). The dependency-cycle gate is green by draft analysis: zero new module arcs (no import changes in any of the four files, no Rust change; sections 11.1/11.2), with the clean-base Step-N record preserved in section 9.3. The Grinch residual observation is adjudicated as a documented, out-of-scope, pre-existing residual (4.1, 6.5, 9.3) and does not contradict this verdict; the implementer must not fix it. The recorded user waiver of the impact-HTML approval pause (9.3) removes no non-user gate; the remaining gates listed in 9.3 execute downstream. The synchronized Spanish impact HTML carries the same verdict. No code implementation or commit is part of this plan task.

## Grinch Review

Step 6 adversarial enrichment (2026-08-22) against the revised Step 4 draft, the preserved working tree (HEAD `45530b46`; exactly two unstaged in-scope files, nothing staged), the Step 8 evidence files, and fresh verification runs on the preserved head.

### Findings (all three incorporated into this revision)

1. **What**: `replaySnapshotFenced`'s snapshot `writeTerminalBytes` is the only write this plan makes that no `try`/`catch` covers, and the function is fire-and-forget (`void replaySnapshotFenced(...)`). **Why**: a synchronous throw from `terminal.write` (2.3: the 50-MiB queue guard throws before queueing) rejects an unawaited promise  -  an unhandled rejection with no warn and no replay-status; pre-fix, the same call ran inside the attach continuation's `try`/`catch` and degraded gracefully into `failSeed`. Reachability is low today (drain + FIFO + single write site empty the queue before the snapshot write) but the plan's own 4.2 throw-safe doctrine and the Errors standard require the same wrap. **Fix**: wrap the snapshot write per 4.2 (warn `"[terminal] attach replay ... failed:"` + stop the generation; no rethrow)  -  incorporated in 4.2/4.4 and section 8 step 3 as an implementer delta.
2. **What**: plan 4.1's invariant ("`inFlight` counts every byte still queued in this entry's xterm FIFO across generations") is not satisfied by the preserved implementation: `flushPendingEventsFenced` queues its replay bytes via `writeTerminalBytes` directly, and post-close live writes bypass the `snapshotReplayPending` gate  -  neither class registers in the drain. **Why**: generation N-1 resolves with a large retained set (up to the 2 MiB reconcile budget; the issue's own near-64-KiB ring class); its flush queues the replay bytes; a fast A→B→A switch (tests 16/17's own scenario) completes attach(N) before xterm's parser drains those bytes; gen N's drain sees `inFlight == 0` and resets, so the older replay bytes parse into the fresh buffer ahead of snapshot(N), which contains the same sequences  -  the exact straddle/duplication defect class the issue forbids ("stale attach/detach generations cannot mutate a newer replay") and plan 6.5 claims impossible ("they stay counted in the shared drain"). **Fix**: register every fenced replay write in the shared drain (unconditional, 0→1 replacement, throw-safe release; the completion callback releases the drain and then runs `finalize` on the last write), so a later generation's fence awaits the older replay bytes; FIFO then also covers post-close live bytes  -  incorporated in 4.1/4.4/6.5 and tested by test 12 sub-step C.
3. **What**: the wheel-guard branch (`drain.userScrolled` → settle skips `scrollToBottom`) is exercised by no test: no test file dispatches a `wheel` event and `userScrolled` appears in none of them; `simulateUserScrollUp` sets `viewportY` directly and never touches the marker. **Why**: plan 10.1 claims the guard protects an intentional scroll "through the attach settle window", but a regression that removes the listener or clears the marker too early would pass all 17 tests while silently overriding user scrolls in the settle window. **Fix**: test 13 gains a settle-window sub-step dispatching `new WheelEvent("wheel")` on `terminal.element` before the settle's rAF and asserting no second bottoming  -  incorporated in 4.5/10.1/10.2.

### Verified (no change needed)

- Plan/HTML hashes matched before edit (`A20E4228197CF1AFF72688CA4E0AEA627828BEE669F7682007109E65D3EF732A` / `E0B2F3AD1339167A07F645E24E828FF374EB437C80FF53D99C63A548D59AB566`); HEAD `45530b46`; exactly two unstaged in-scope files; nothing staged; no stash; no commit.
- All section 5.1 line references match the preserved working tree exactly (including `buffer.active` at 819); base file sizes 900/618 match 2.1; no imports added; `scrollToBottom` has exactly one call site (816); `terminal.write` has exactly one call site.
- The two compatibility corrections are minimal and sufficient: render-gate's synchronous optional-callback invocation + constant `buffer.active` makes its two failing tests pass with assertions unchanged (traced assertion-by-assertion) and cannot disturb its other 6 tests; App.workflow's constant buffer removes the 4 unhandled TypeErrors at 819 with zero assertion and zero `pty_resize`-count impact (the settle already issued its resize before throwing today).
- Fresh full-suite run on the preserved head reproduced the blocker baseline exactly: **2 failed / 1599 passed (1601), 4 errors**  -  both failures the render-gate replay tests (`expected +0 to be 1` on `resets`), all 4 errors the App.workflow settle TypeError at `TerminalView.tsx:819`. Focused attachment 17/17 green; `typecheck` PASS; `check:frontend-dependencies` PASS (344 modules, 0 errors); `build` PASS (pre-existing warnings only).
- Verification contract: CI pins Node 22 + npm 11.6.2; `frontend-regression` runs typecheck + `npm test` with the CI-owned #480 known-debt guard; the `rust-regression` jobs run `npm run build`; no lint script and no ESLint dependency exist; no Rust file changes (git status proves it); the Step-N detector path and `scripts/02-module-arc-record.mjs` exist.
- Six-path commit contract: the plan artifacts are untracked and gitignored (`/plans/`), consistent with force-adding them only at the final commit; no planning commit exists (HEAD == base).
- Waiver: message `20260822-050820` recorded; plan 9.3 preserves every non-user gate (report, idle, freeze, digest, purge, cold-start, review, current-main, landing, ship) and removes only the approval pause.
- Fake inventory complete: exactly 7 xterm-mocking test files; only render-gate and App.workflow are incompatible; every other test file referencing TerminalView/App mocks xterm (no unmocked attach paths).

Verdict: plan corrected per findings 1-3; `Status: DRAFT_FOR_ENRICHMENT` unchanged; not certified READY.

### Architect Step 7 adjudication (2026-08-22)

The architect Step 7 consensus returned `NEEDS_ANOTHER_ROUND`: finding 1 (throw-safe snapshot write) and finding 3 (wheel-guard coverage) were adjudicated correct and complete; finding 2 (shared-drain registration) was adjudicated necessary but incomplete - the snapshot write itself (4.4 step 6) was a third write class left unregistered, and test 12's main body encoded the un-fixed ordering. This round completes finding 2:

- 4.1 enumerates all three shared-drain write classes - the window-registered live writes of 4.2, the snapshot write of 4.4 step 6, and the fenced replay writes of 4.4 step 4 - and specifies the snapshot registration with the same 0→1 promise replacement, `inFlight` increment, and throw-safe release (4.2), its completion callback releasing the drain and then running the flush trigger.
- 4.4 step 6 composes the snapshot write's drain registration with its throw-safe wrap: a synchronous throw releases the drain with the 4.2 warn and stops the generation (never an unhandled rejection), and the registration makes a later generation's fence await an older generation's still-queued snapshot bytes before its reset.
- 6.5's shared-drain coverage sentence now includes all three classes (snapshot write included).
- 10.2 test 12's main body is re-sequenced: generation 2's snapshot write is asserted NOT queued while generation 1's gated snapshot callback is held (the corrected fence holds it, the snapshot-class variant of sub-step A); generation 1 then completes, generation 2's snapshot write queues and settles once, and generation 1's flush is inert. The existing live/replay sub-steps (A and C) are retained; test count stays 17.
- 8 step 3 names all three implementer deltas (throw-safe snapshot write; snapshot-write drain registration; fenced-replay drain registration).
- All 17 U+2014 characters are normalized to ` - ` (final count: zero).

### Architect Step 7 consensus, round 2 (2026-08-22)

Round 1 returned `NEEDS_ANOTHER_ROUND` (architect message `20260822-070237`); dev round 2 (report `20260822-070714`) applied all seven corrections; Grinch round 2 (report `20260822-071549`) verified them adversarially (release discipline exactly once with mutually exclusive release paths; release-then-flush ordering load-bearing; no deadlock, double release, or stranding; test-12 main body traced end-to-end; no forbidden additions), applied one consistency edit (section 12: "two production deltas" to "three"), and raised a residual observation. The architect adjudicates that residual as documented, out of scope, and pre-existing (recorded in 4.1 and 6.5): a post-close live byte queued after the newest counted write immediately before a sub-12-ms A→B→A switch. The three-class shared-drain correction and the deterministic test-12 re-sequencing are adjudicated complete and correct. This plan is certified `READY_FOR_IMPLEMENTATION` at the round-2 consensus (sections 9.3 and 12).
