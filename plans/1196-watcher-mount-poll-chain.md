# Implementation Plan: #1196 The watcher window can hang before it ever arms its poll

Status: READY_FOR_IMPLEMENTATION

Full path. Written by the architect at Step 4, enriched by `dev-webpage-ui` (Step 5) and `dev-rust-grinch` (Step 6), certified by the architect at Step 7 round 1 after resolving every finding both raised, and recertified after a narrow follow-up check of the two shapes round 1 introduced. **Section 12 is the record of both.** Where Section 12 changed an earlier section, that section was rewritten too, so the two agree; read Section 12 for why a settled question is settled, not for instructions.

Every implementation decision is closed. There is no `TBD`, no competing alternative and nothing left to the implementer, who is expected to start cold with no knowledge of the discussion that produced this. Section 11 lists what could not be verified, which is not the same thing as an open decision.

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1196 (`The watcher activity window can hang on mount and never arm its poll`).
- Branch: `fix/1196-watcher-mount-poll-chain`, already created and checked out.
- **Baseline for every coordinate and every command in this plan: `fd90894136c5096b087ed8b32eed53f76df83112`** (`Merge pull request #1221 from mblua/fix/1216-ci-rust-cache-and-smoke-bundling`). Verified independently three times: by the architect at Step 4, by `dev-webpage-ui` at Step 5 and by `dev-rust-grinch` at Step 6. `git rev-parse HEAD` returns `fd908941`, the branch carries no commits of its own, and `git status --porcelain` is empty, so every `file:line` below is valid at branch HEAD.
- Delivery classification: FULL. Confirmed, not reclassified. Three properties make it non-mechanical: more than one shape is defensible for where the poll gets armed; the failure is silent, so a half-fix leaves the window looking healthy; and the change reverses a documented mount invariant that an existing test pins (Section 2.6).

**Objective.** The watcher activity window must always reach a state where it is polling, whatever happens to any single startup call, including a call that never settles at all. When startup does not complete, the window must say so on screen and keep working in the degraded state rather than sit silently on `Waiting for the first sample...`.

**Non-objective.** This is not an investigation into why a Tauri command might stop replying, and it is not a Rust change. It is not a refactor of the poll, of `refresh()`, or of the window's state model. It does not attempt to retry what failed.

## 2. Verified current state

Everything in this section was read at `fd908941`. Line numbers are current.

### 2.1 The mount chain, at exact coordinates

`onMount` opens at `src/watchers/App.tsx:477`, its `try` body runs to `:541`, and its `catch` is `:542-550`. Inside it, eight sequential `await`s:

| # | Await | Line | Underlying IPC | Local failure handling today |
|---|-------|------|----------------|------------------------------|
| 1 | `settingsStore.load()` | `:481-483` | `invoke("get_settings")` | `.catch(...)` at `:483`, logs and continues |
| 2 | `register(onWatcherMatches(...))` | `:489-497` | `invoke("plugin:event\|listen")` | none, a rejection ends the mount |
| 3 | `register(onWatchersScopeRequest(...))` | `:498-505` | `invoke("plugin:event\|listen")` | none, a rejection ends the mount |
| 4 | `register(onSessionCreated(...))` | `:506` | `invoke("plugin:event\|listen")` | none, a rejection ends the mount |
| 5 | `register(onSessionDestroyed(...))` | `:507` | `invoke("plugin:event\|listen")` | none, a rejection ends the mount |
| 6 | `register(onSessionRenamed(...))` | `:508` | `invoke("plugin:event\|listen")` | none, a rejection ends the mount |
| 7 | `reloadSessions()` | `:510` | `invoke("list_sessions")` | own `try/catch` at `:382-389`, logs and continues |
| 8 | `WindowAPI.getWatchersScope()` | `:520` | `invoke("get_watchers_scope")` | own `try/catch` at `:519-526`, logs and continues |

A ninth await, `trackGeometry()` at `:536`, sits **after** the arming point and is not part of this defect. Section 3 keeps it that way deliberately.

**All eight are IPC round trips.** The five `register(...)` calls read as local subscriptions and are not: `transport.listen` (`src/shared/ipc.ts:118-119`) reaches `TauriTransport.listen` (`src/shared/transport-tauri.ts:28-34`), which is `await this.ready; return this.listenImpl!(...)`, and `@tauri-apps/api`'s `listen` is itself an `invoke('plugin:event|listen', ...)` (`node_modules/@tauri-apps/api/event.js:74-81`). `TauriTransport` has no timeout on any path: `invoke` is `await this.ready; return this.invokeImpl!(cmd, args)` (`transport-tauri.ts:23-26`). Worse, `this.ready` is the dynamic import in `init()`, so a stall there stalls every call the window makes.

### 2.2 The arming point, and why it is the whole defect

- `setScopeSettled(true)` is `App.tsx:529`.
- `schedulePoll()` is `App.tsx:530`.

`schedulePoll` is defined at `:465-475` and has exactly two call sites: `:456`, inside `runPollRound`'s `finally`, reachable only from the timer `schedulePoll` itself arms at `:469-474`; and `:530`. The chain is self-sustaining but not self-starting, so **`:530` is the sole entry into the poll chain** and **`:529` is the sole writer of `scopeSettled`**. Both sit behind all eight awaits.

`scopeSettled` also gates the first fetch: the effect at `:413-435` returns immediately at `:416` while it is false, so `void refresh()` at `:434` never fires either. One hung await therefore takes down both the first fetch and the poll.

### 2.3 The failure is silent by construction

`loadError` has exactly two writers, `App.tsx:365` (inside `refresh()`'s catch) and `App.tsx:549` (inside the mount's catch). On a hang:

- `:549` is unreachable: a promise that never settles never rejects, so the `catch` at `:542` never runs.
- `:365` is unreachable: `refresh()` is only called from `:434` and from `runPollRound` at `:449`, and both are behind `scopeSettled`/`schedulePoll`.

So `loadError()` stays `""`, `<Show when={loadError()}>` at `:758` renders nothing, `snapshots()` stays `[]`, `resolveView([], 0, 0)` returns `"warming"` (`src/watchers/activity.ts:290-292`), and the window paints `Waiting for the first sample...` (`App.tsx:794-798`) for as long as it is open.

### 2.4 `withDeadline` as it stands

`src/watchers/App.tsx:157-169`, doc comment `:142-156`:

```ts
export function withDeadline<T>(work: Promise<T>, ms: number, message: string): Promise<T>
```

- Takes an **already-created promise**, not a thunk, so wrapping does not defer or start the work.
- `Promise.race([work, deadline])`, where `deadline` rejects with `new Error(message)` after `ms` (`:163-165`).
- On timeout it **rejects**; it does not cancel `work`. The doc comment states this outright at `:145-147`.
- `Promise.race` keeps a handler attached to `work`, so a late rejection of an abandoned call is absorbed instead of escaping as an unhandled rejection (`:150-152`).
- The timer is cleared on both paths (`:166-168`), so a settled call leaves nothing armed.
- **Per-element, never cumulative.** Each call gets its own fresh `ms`.

`grep -rn "withDeadline" src/` at `fd908941` returns: the definition (`App.tsx:157`), **one** production call site (`App.tsx:349`, inside `refresh()`), a doc reference (`App.tsx:442`), and three test references (`App.test.tsx:10, 994, 1000`). **Nothing on the mount path is wrapped.**

### 2.5 The two hypotheses inherited from Step 1, cross-checked against the code

Both came from `dev-webpage-ui` labelled as opinion. Both were re-checked here rather than adopted.

**Hypothesis 1, "a deadline alone is a half-fix": UPHELD.** Traced at `fd908941`: wrapping the eight awaits and changing nothing else means a timeout rejects, the rejection leaves the `try` at `:478`, the `catch` at `:542` sets `loadError` and the mount ends. `:529-530` are never reached, so the window gains a banner and stays dead. Arming must survive a failed await.

**Hypothesis 2, "awaits #1, #7 and #8 are already best-effort, which may point at a cheaper fix": OVERRULED as a scoping argument.** The asymmetry is real (their catches are at `:483`, `:386-389` and `:523-526`) but it is about **rejection**, not about **hang**. A catch handler never runs for a promise that never settles, so leaving #1, #7 or #8 unbounded leaves three of the eight hazards fully open. There is no cheaper subset: all eight must be bounded. The asymmetry is still useful, but for a different question, namely what should happen *after* a bound fires, and Section 4.5 uses it there.

### 2.6 What the current tests pin, including one that this change must break

- `App.test.tsx:817-837` ("reports a failed setup instead of leaving an unhandled rejection behind") makes await #6 **reject** and asserts a visible `watchers.error` banner containing the message **and** `expect(fake.callsFor("get_watcher_activity")).toHaveLength(0)`, i.e. that the mount stopped where it stood. That second assertion encodes the invariant this plan deliberately reverses. **The test must be updated as part of the change** (Section 5.2). It is not collateral damage; it is the old contract written down.
- `App.test.tsx:844-863` ("stops the mount where it is when the window closes mid-flight") pins that after teardown nothing is fetched and no poll is armed. **This test must keep passing unchanged**, which constrains the design (Section 4.5).
- `App.test.tsx:982-1020` and `:1031-1229` are the #1188 suites. They cover the running chain via `refresh()` only. One shared helper in them, `flushMount()` at `:1061-1063`, must be changed; Section 4.9 is the whole of that question.
- **No test exercises a never-settling mount await.** That is the coverage gap.

### 2.7 Written precedent in this repository

- **#1188 solved the same shape one level down.** `runPollRound` (`:447-463`) puts the re-arm in a `finally` and the fetch behind a `withDeadline`, and the doc comment at `:437-446` says why both are needed: `finally` alone does not help, "because `finally` does not run for a promise that never settles either". This plan applies the identical two-part remedy to the mount.
- **Best-effort failures are logged, not painted.** `App.tsx:483`, `:388`, `:525`, `:580`, `:608` all use `console.error` with a `[watchers]` prefix.
- **The window already carries non-error notices.** `watchers.truncated` (`:766-770`), `watchers.missedFrames` (`:771-775`) and `watchers.degraded` (`:780-786`) are three sibling banners with existing CSS (`src/watchers/styles/watchers.css:164-189`). A fourth is a pure insertion, not a new pattern.
- **A constant plus an exported message is the established way to make a timeout testable.** `POLL_TIMEOUT_MS` (`:66`) and `POLL_TIMEOUT_MESSAGE` (`:70-71`) exist for exactly that, per their doc comments.

### 2.8 Scope limiter: this is Tauri-desktop only, and more narrowly than it looks

`createDefaultTransport` (`src/shared/ipc.ts:89-91`) is `isTauri ? new TauriTransport() : new WsTransport()`, and `isTauri` (`src/shared/platform.ts:3-4`) is a module-level const, `"__TAURI_INTERNALS__" in window`.

`WsTransport` is already bounded: `invoke` rejects at 30 s (`src/shared/transport-ws.ts:255-260`) after a `waitForConnection` capped at 5 s (`:222-239`), and `listen` is purely local with no await at all (`:264-285`). **The browser path cannot hang indefinitely.**

Narrower still: **the shipped router cannot put `WatchersApp` on the WS transport at all.** `src/main.tsx:35-36` renders `BrowserApp` for every non-Tauri page, and the `windowType === "watchers"` branch is only reached at `:47`, after that. So today there is no WS watchers path in production to regress. Section 4.6 says what follows from that.

## 3. Scope

### In scope

1. Bounding all eight mount awaits in `src/watchers/App.tsx` under **one shared budget** for the whole chain.
2. Making the arming of the poll (`setScopeSettled(true)` and `schedulePoll()`) structurally unconditional for every await that has been bounded, so that no outcome of any bounded mount await can prevent it.
3. Making every mount await survivable, so the chain always runs to its end and always arms.
4. Splitting `reloadSessions` so the mount can observe a `list_sessions` **rejection**, which today it structurally cannot (Section 4.5).
5. Single-flighting this window's `get_settings` calls, so arming the poll cannot accumulate one never-settling `invoke` per period (Section 4.10).
6. One new persistent on-screen notice saying that startup did not complete, plus its three exported constants.
7. Updating `src/watchers/App.test.tsx`: one existing test rewritten to the new contract, one shared helper's turn count raised, and a new suite covering the never-settling mount await.

### Out of scope, and why

- **`trackGeometry()` (await #9, `:536`) is not bounded.** It runs after the arming point, so a stall there costs a persisted window rect and nothing else. Bounding it would widen the change for no user-visible gain.
- **`reloadSessions()` called from the three session listeners (`:506-508`) is not bounded.** Those calls are `void`-fired from event handlers and block nothing; a stall there costs a stale session list, not the poll chain. Only the mount's own call at `:510` is bounded, at the call site, so this stays exactly as wide as #1196.
- **`withDeadline` is not changed.** Neither its signature nor its semantics. See Section 4.4.
- **`src/shared/stores/settings.ts` is not changed.** The single-flight guard of Section 4.10 lives in `App.tsx` and binds only this window, because the store is shared with the sidebar, the terminal and the main window, and a global single-flight would silently change refresh semantics for consumers this issue has not analysed.
- **`src/shared/transport-ws.ts` and `src/shared/transport-tauri.ts` are not changed.** No transport-level timeout is added, moved or removed.
- **No Rust change, no IPC surface change.** No deadlock was established in the backend command graph (Section 11.1). This is a frontend resilience fix that is correct whether or not a backend stall exists.
- **No retry of a failed startup step.** Nothing re-issues `plugin:event|listen`, `list_sessions` or `get_watchers_scope` after the mount. The poll's own periodic `get_watcher_activity` and `get_settings` are the window's normal operation, not a retry of the mount, and Section 4.10 bounds what that costs.
- **Abandoned calls are not cancelled.** Nothing can cancel a Tauri `invoke`; #1188 already accepted this and documented it at `:145-147`.
- **The truly-lost-reply Tauri listener residual is documented, not closed** (Section 6.5).

## 4. The decided solution

### 4.1 Shape, in one paragraph

Every one of the eight mount awaits goes through a local `step()` helper. `step()` does two things: it races the work against **the time remaining in a single mount-wide budget**, and it catches any failure that is not teardown, logs it, raises a `startupDegraded` flag and returns `undefined` so the chain continues. The eight steps are wrapped in an inner `try/finally` whose `finally` performs the arming. The result is that the mount always reaches its arming point within roughly one budget, on every path, and the window says on screen when it got there degraded.

This is the #1188 remedy applied one level up: a deadline to turn "never settles" into "settles as a failure", and a `finally` to make the re-arm unconditional once everything pending has been made to settle.

### 4.2 Decision 1: how the poll chain gets armed

**Decided: arming moves into a `finally` that wraps only the eight-await chain, guarded by `if (!disposed)`.**

```ts
try {
  // ... the eight steps ...
} finally {
  // Arming is the first thing after the chain and runs on every path out of it.
  if (!disposed) {
    setScopeSettled(true);
    schedulePoll();
  }
}
```

**The guarantee is two-part, and stating only half of it is false.** A `finally` does not run for a promise that never settles, so `finally` alone protects nothing. The invariant this design maintains is:

> **(i)** every await inside the inner `try` goes through `step()`, so nothing in it can stay pending forever; **and (ii)** the `finally` arms after whatever settlement or throw results.

Neither half is sufficient alone. In particular, a future edit that adds a ninth **unwrapped** await inside the inner `try` *can* reintroduce this defect, because a hang there never reaches the `finally` at all. That is why AC5 checks part (i) mechanically rather than trusting the shape, and why this paragraph exists instead of the shorter claim that a `finally` makes the guarantee structural on its own.

Why the `finally` wraps **only** the eight-await chain and not the whole mount body. `trackGeometry()` at `:536` is a ninth await, and it is currently after the arming. If the `finally` wrapped the whole `try`, arming would move *behind* `trackGeometry()` and a stall in the dynamic `import("@tauri-apps/api/window")` at `:558`, which sits outside `trackGeometry`'s own `try` at `:591`, would recreate this exact defect at a new position. The inner `try/finally` keeps arming strictly before geometry. Note that the geometry block is itself gated by `if (isTauri)` at `:532`, so this hazard exists only on the desktop path, which is the exposed one; the argument is stronger than it looks, not weaker.

Why `if (!disposed)`. `setScopeSettled(true)` and `schedulePoll()` must not run after teardown. `schedulePoll` already self-guards at `:466`, but `setScopeSettled` does not, and the test at `App.test.tsx:844-863` asserts that nothing is fetched after cleanup. The guard is what keeps that test green. `onCleanup` sets `disposed` synchronously at `:614` and there is no `await` between the guard and `schedulePoll()`, so the check cannot be raced.

Order inside the `finally` is unchanged from `:529-530`: `setScopeSettled(true)` first, `schedulePoll()` second. `schedulePoll()` only arms a timer and issues nothing, so its position cannot lose a round, and keeping the existing order avoids an unnecessary behavioural delta.

**What happens to `scopeSettled` when the scope fetch is the thing that failed.** `scopeSettled` means "the mount has decided the scope", not "the scope was fetched successfully". That is already the shipped semantics: today a *rejecting* `getWatchersScope()` is caught at `:523-526`, logged, and `:529` sets `scopeSettled` anyway, leaving the query-parameter scope standing, which `:514-517` documents as deliberate. **A timed-out scope fetch behaves identically to a rejecting one.** The scope is `props.initialSessionId ?? null` (`:195-197`), which is the scope the window was opened with. No new semantics are introduced.

### 4.3 Decision 2: which awaits are bounded, with what budget

**Decided: all eight, under one shared budget of 10 000 ms, expressed as remaining time per call.**

All eight, because per Section 2.5 there is no subset that closes the defect.

**Shared, not per-element.** A per-element deadline on a sequential chain multiplies: eight awaits at 8 s each is 64 s of `Waiting for the first sample...` before the window arms, and a real backend stall makes every remaining await time out in turn, so the multiplication is the expected case rather than the pathological one. The budget is therefore an absolute expiry taken once, at the top of the mount:

```ts
const budgetEndsAt = Date.now() + MOUNT_TIMEOUT_MS;
```

and each call is raced against what is left of it: `Math.max(0, budgetEndsAt - Date.now())`. Steps that inherit an exhausted budget get a zero-length deadline; Section 6.3 states exactly what that does, because it is not simply "fail fast".

**The number, and the arithmetic. Read the qualifiers: this is a service target under a responsive event loop, not a proof of a hard bound.**

- Break-even: with eight sequential round trips sharing 10 000 ms, the budget fires once the **total elapsed time** of the chain passes 10 000 ms, which is an **average** of `10_000 / 8 = 1_250 ms` per step *if the eight account for the whole elapsed budget*. It is **not** a per-call allocation: step #1 can consume nearly the entire 10 000 ms on its own, and any early step can be allowed more than `POLL_TIMEOUT_MS`. The elapsed time also includes JS work and microtask turns, not only IPC latency.
- **No measurement exists for any of the eight startup commands.** The only in-repo datum is the one at `App.tsx:61-62`, **13 ms worst stressed for `get_watcher_activity`**, which is a different command. Quoted here as the only available order-of-magnitude anchor and nothing more: eight operations at that figure is 104 ms, about 1% of the budget.
- The load-bearing headroom figure is the pessimistic one: at a deliberately pessimistic 500 ms per call, eight calls is 4 000 ms and the budget still has 2.5x headroom.
- Ceiling: the budget must not exceed one focused poll period, `POLL_FOCUSED_MS = 10_000` (`:52`), because a window that is not polling by the time its first period would have elapsed has stopped being a live view. 10 000 sits **at** that ceiling deliberately; the ceiling is the constraint, and the value is chosen to take all of it because the cost of firing early (a healthy-but-slow start loses listeners and gets a notice) is worse than the cost of firing late (the user waits).
- Floor: the budget must exceed `POLL_TIMEOUT_MS = 8_000`, the allowance one activity round gets for a single call, since the mount has eight calls to make. `10_000 > 8_000`. This is the strict half of the invariant.
- Worst case from mount start to arming is therefore `MOUNT_TIMEOUT_MS` plus the settle overhead of the remaining zero-budget steps, **under a responsive event loop**. `setTimeout` supplies a minimum delay and not a maximum, so a blocked or throttled event loop can arm arbitrarily later; no wording in this plan should be read as a hard upper bound.
- **The invariants to pin in a test** are `MOUNT_TIMEOUT_MS > POLL_TIMEOUT_MS` (strict, the floor) and `MOUNT_TIMEOUT_MS <= POLL_FOCUSED_MS` (**non-strict**, the ceiling). Equality with `POLL_FOCUSED_MS` is the intended value, so the ceiling assertion must not be written as `<`.

**Why the ceiling is non-strict, when the analogous-looking `App.test.tsx:1015` is strict.** `:1015` asserts `POLL_TIMEOUT_MS < POLL_FOCUSED_MS`, and its comment at `:1011-1014` justifies strictness by service level: rounds are chained, so a deadline at or above the period means a hung round is reported no sooner than the next round would have been due. **That argument does not transfer to the mount, because the mount has no cadence behind it.** Nothing is due at t=10 000 that the budget could collide with: the first fetch is issued at arming, whenever that is, not on a schedule. The mount ceiling is a statement about how long a user may look at a window that is doing nothing, which is a different claim from `:1015`'s, and equality is a legitimate reading of it. `App.test.tsx:1018` (`POLL_FOCUSED_MS <= POLL_UNFOCUSED_MS`) is cited nowhere in this plan as an analogy, because it is a cadence ordering and is not one.

`Date.now()` is safe under the test harness: `vi.useFakeTimers()` replaces it, which `App.test.tsx:1049-1050` states as a known property of this suite. It is wall-clock rather than monotonic; Section 6.6 states the accepted consequence.

### 4.4 Decision 5: `withDeadline` does not change

**Decided: no change to the helper, no change to its one existing call site.**

The shared budget is expressed by the **caller** computing the remaining milliseconds, so `withDeadline(work, ms, message)` keeps its exact signature and semantics. This is what makes the cumulative budget cheap: no new overload, no thunk parameter, no cancellation contract.

**Regression audit of the existing call site.** `App.tsx:349`, inside `refresh()`, passes `POLL_TIMEOUT_MS` and `POLL_TIMEOUT_MESSAGE`, both untouched. It sits inside `ids.map(...)` deliberately per session (rationale `:342-346`) and neither the mount budget nor `step()` is reachable from it. The mount constants are separate constants used only on the mount path. After the change, `grep -rn "withDeadline" src/` must show the definition, `:349`, the new use inside `step()`, the doc reference at `:442` and the test references, and nothing else. That grep is an acceptance criterion (Section 9.2), but on its own it proves only that no *extra* production use appeared; AC5 is what proves all eight positions go through the bound.

### 4.5 Decision 4: mount error semantics, stated precisely

**Decided: no step of the eight-await chain ends the mount. Teardown, and only teardown, ends the mount.**

One rule, no special cases: **any startup step that does not complete, for any reason other than the window closing, is logged, raises the startup-degraded flag, and the chain continues to the next step.** The rule is the same for a rejection and for a timeout, because the user-visible consequence is the same: that piece of startup did not happen.

This deliberately reverses the invariant documented at `App.test.tsx:812-815` and asserted at `:833`. The reversal is the point of the issue: a window that ends its mount is a window that never polls, and a degraded window that polls every 10 s is strictly more useful than a dead one with a red banner. The "say so" half of that old contract is preserved and in fact strengthened, because the new notice is persistent where `loadError` is not (Section 4.7).

**Await #7 needs a code change to obey the rule, not just a wrapper.** `reloadSessions` (`:380-390`) catches its own rejection at `:386-389`, logs, and **fulfils**. Wrapping a fulfilled promise in `step()` cannot observe the failure, so a *rejecting* `list_sessions` would raise no notice at all, and in "All agents" scope that reproduces #1196's exact symptom: `scopeIds()` is `[]` (`:220-224`), `refresh()` awaits `Promise.all([])`, `setSnapshots([])` at `:357`, `resolveView([],0,0)` is `"warming"`, and the window paints `Waiting for the first sample...` with no banner. The function is therefore **split**: a primitive that fails honestly, and a best-effort wrapper for the three event-driven callers.

```ts
/**
 * List the sessions and adopt the answer, REJECTING if the call fails.
 *
 * #1196: split from `reloadSessions` because the mount has to see a failure in order to
 * report it, while the three session listeners want it swallowed. One function that catches
 * internally cannot serve both, and the version that did is exactly how a rejecting
 * `list_sessions` reached the user as a blank window with no banner at all.
 *
 * The stale guard is applied to the failure as well as to the success: an answer that is no
 * longer the newest is not this window's problem, whichever way it went.
 */
const loadSessions = async (): Promise<void> => {
  const request = (sessionsRequestCounter += 1);
  let listed: Session[];
  try {
    listed = await SessionAPI.list();
  } catch (err) {
    if (disposed || request !== sessionsRequestCounter) return;
    throw err;
  }
  if (disposed || request !== sessionsRequestCounter) return;
  setSessions(listed);
};

/** Best-effort for the three session listeners, which must not fail an event handler. */
const reloadSessions = async (): Promise<void> => {
  try {
    await loadSessions();
  } catch (err) {
    console.error("[watchers] failed to list sessions:", err);
  }
};
```

The mount then uses `await step(loadSessions(), "list sessions")`, and `:506-508` keep calling `void reloadSessions()` unchanged.

Consequences per await, all of them intended:

| # | Await | Today on rejection | Today on hang | After the change |
|---|-------|--------------------|---------------|------------------|
| 1 | `settingsStore.load()` | logged, mount continues | mount hangs forever | logged, **notice**, mount continues |
| 2-6 | `register(...)` | banner, **mount ends** | mount hangs forever | logged, **notice**, mount continues |
| 7 | `loadSessions()` | logged, mount continues, **no notice** | mount hangs forever | logged, **notice**, mount continues |
| 8 | `getWatchersScope()` | logged, mount continues | mount hangs forever | logged, **notice**, mount continues |

Await #1's inline `.catch(...)` at `:483` is **removed**, because `step()` now handles it and leaving both would swallow the failure before the notice could see it. Await #8's inline `try/catch` at `:519-526` is **replaced** by `step()`, which returns `undefined` on failure; the generation guard at `:518` and `:522` is preserved verbatim and `if (pulled && ...)` already handles `undefined`.

**Teardown must still end the mount, and must not be reported as degradation.** `register` throws the `mountDisposed` sentinel (`:182`, `:188`) when the window closed under it. `step()` must **rethrow** when the caught value is that sentinel or when `disposed` is true, so the sentinel reaches the outer `catch` at `:547` and is recognised there as it is today. The three existing `if (disposed) return;` checks at `:484`, `:511` and inside the scope-pull block stay exactly where they are; combined with the `if (!disposed)` guard in the `finally`, they are what keeps `App.test.tsx:844-863` green.

**The outer `try/catch` at `:478`/`:542-550` stays** as the belt for everything outside the eight steps, chiefly the geometry block at `:532-541`. Its behaviour is unchanged: sentinel or disposed returns, anything else logs and sets `loadError`. `MOUNT_TIMEOUT_MESSAGE` can therefore never be painted: the only route from `step()` to the outer catch requires `disposed`, and that catch returns on `disposed` before touching `loadError`.

### 4.6 Decisions 3 and 6: what the user sees, and the transport question

**There is no i18n layer in this codebase.** Verified: no `src/i18n` or `src/locales` directory, no `i18next`, no `useTranslation`, and no match for `i18n|intl|translat|lingui` in `package.json`. Every string in `src/watchers/App.tsx` is an inline English literal, and `watchers.error` is a `data-ac-testid`, not a message key. **No i18n key is reused and none is created.** The three new strings follow the established pattern of `POLL_TIMEOUT_MESSAGE` (`:70-71`): module-level exported constants, so a test can assert the exact string the window paints instead of a substring that could drift.

**Per failure mode, what the user sees:**

| Failure | On screen | In the console |
|---|---|---|
| Any single startup step times out | the startup notice, persistently, plus a working window that polls | `[watchers] mount step "<what>" did not complete: Error: <MOUNT_TIMEOUT_MESSAGE>` |
| Any single startup step rejects | identical to the above | the same line carrying the underlying error |
| Every startup step succeeds | nothing new | nothing new |
| The window is closed mid-mount | nothing, and no poll is armed | nothing |
| A poll round fails after startup | `watchers.error`, unchanged from #1188 | unchanged |

The raw error text goes to the console rather than to the banner, which is what `:483`, `:388` and `:525` already do for every best-effort failure in this file. The user cannot act on `transport closed mid-subscribe`; they can act on `close and reopen the window`, and reopening is a real gesture (`WindowAPI.openWatchers`, `src/shared/ipc.ts:538-539`, fired from `src/terminal/components/StatusBar.tsx:66`).

**Why a new notice rather than reusing `loadError`.** `loadError` cannot carry this message: the scope effect clears it at `:433` on every scope change, and every successful `refresh()` clears it at `:358`. Since the poll runs every 10 s, anything written there is erased within roughly one period of the window arming. A message that disappears 10 s after startup fails is not a report.

**Decision 6, the transport question.** The budget applies on both transports, with no `isTauri` branch, and `transport-ws.ts` is not modified. Four reasons:

1. **Bounds compose as a minimum.** Two bounds on one call do not conflict, do not stack and do not both fire; the tighter one wins and the looser stays as the backstop for everything the UI does not wrap. There is no "double-bounding" hazard to avoid.
2. **Shipped precedent for a transport-agnostic UI deadline.** #1188 put an 8 s `withDeadline` on `get_watcher_activity` (`:349`) with no transport branch, against a WS invoke bound of 30 s. A 10 s mount budget is the same policy, one notch looser.
3. **There is no shipped WS watchers path to regress.** `src/main.tsx:35-36` renders `BrowserApp` for every non-Tauri page, before the `windowType === "watchers"` branch at `:47`, so production `WatchersApp` never constructs a `WsTransport`.
4. **A transport branch inside a window component would be a new pattern.** No component in `src/watchers/` reads `isTauri` except the geometry gate at `:532`, which exists because the Tauri window API is genuinely absent elsewhere. Branching a timeout on the transport is a different thing and this change is not the place to introduce it.

Retained for completeness rather than as a live constraint: **were that router branch ever added, the budget would still be safe on WS.** `WsTransport.listen` is local with no await (`transport-ws.ts:264-285`), so five of eight steps cost ~one microtask; only #1, #7 and #8 are round trips, over a websocket bound to `127.0.0.1` by default. Each invoke calls `waitForConnection()` independently (`:222, :242`), but it returns an already-resolved promise once `readyState === OPEN`, so only the first can pay the 5 s. Worst healthy case is roughly 5 s plus three localhost round trips, about half the budget.

### 4.7 The three new module-level constants and the new signal

```ts
/** How long the whole mount chain may take before what is left of it is abandoned.
 *
 *  #1196: shared across all eight startup awaits, not per await. A per-await deadline on a
 *  sequential chain multiplies, and a backend that has stopped answering makes every
 *  remaining await time out in turn, so eight awaits at one deadline each is the expected
 *  case rather than the pathological one. Expressed as an absolute expiry taken once, so the
 *  worst case from mount to arming is this value plus change rather than eight times it,
 *  under a responsive event loop. It is a service target, not a hard bound: `setTimeout`
 *  gives a minimum delay, and `Date.now()` is wall-clock.
 *
 *  It is NOT a per-call allowance. Step #1 can consume nearly all of it. 10s over eight
 *  sequential operations fires once their TOTAL passes it, i.e. an average above 1250ms if
 *  the eight account for the whole elapsed budget. It exceeds POLL_TIMEOUT_MS because eight
 *  calls need more than the one call a poll round makes, and it does not exceed
 *  POLL_FOCUSED_MS because a window that is not polling by the time its first period would
 *  have elapsed has stopped being a live view. Exported for the invariant test. */
export const MOUNT_TIMEOUT_MS = 10_000;

/** What `withDeadline` rejects with when the mount budget runs out. Logged, never painted:
 *  the per-step context is in the log prefix and the user-facing statement is
 *  STARTUP_DEGRADED_MESSAGE. */
export const MOUNT_TIMEOUT_MESSAGE = "The window did not finish starting up in time.";

/** What the window says when any startup step did not complete. Persistent, because the
 *  window cannot tell which of the losses will repair themselves and which will not, and
 *  reopening is the only reliable retry. Exported so the regression test asserts the exact
 *  string the window paints. */
export const STARTUP_DEGRADED_MESSAGE =
  "This window did not finish starting up, so some updates may be missing. Close it and open it again to retry.";
```

```ts
/** True once any startup step failed or was abandoned. Never cleared: see
 *  STARTUP_DEGRADED_MESSAGE. */
const [startupDegraded, setStartupDegraded] = createSignal(false);
```

### 4.8 The `step()` helper

Local to `onMount`, because it closes over the budget and over `disposed`:

```ts
/**
 * One step of the mount: bounded by the shared budget, and survivable.
 *
 * #1196: the deadline is what turns "never settles" into "settles as a failure", and
 * swallowing that failure is what lets the chain reach its arming point. The two are one
 * decision: a bound whose rejection still ends the mount only converts a silent dead window
 * into a loud one.
 *
 * `disposed` is the one thing that is NOT a degradation. `register` throws `mountDisposed`
 * when the window closed under it, and that has to keep ending the mount, so it is rethrown
 * rather than reported.
 */
const step = async <T,>(work: Promise<T>, what: string): Promise<T | undefined> => {
  try {
    return await withDeadline(
      work,
      Math.max(0, budgetEndsAt - Date.now()),
      MOUNT_TIMEOUT_MESSAGE
    );
  } catch (err) {
    if (err === mountDisposed || disposed) throw err;
    console.error(`[watchers] mount step "${what}" did not complete:`, err);
    setStartupDegraded(true);
    return undefined;
  }
};
```

Notes the implementer must not lose:

- The generic parameter is written `<T,>` with the trailing comma. This is a `.tsx` file and `<T>` alone parses as JSX.
- `work` is evaluated **before** `step` is entered, which is the point: the IPC call is issued regardless, and only the wait is bounded. A step that inherits a zero budget still *issues* its call.
- A late rejection from an abandoned `work` is absorbed by `Promise.race`'s still-attached handler inside `withDeadline` (`:150-152`), so an abandoned `register` that later throws `mountDisposed` does not escape as an unhandled rejection.
- An abandoned `register` **whose reply eventually arrives** pushes its unlisten into `listeners` at `:190` if the window is still alive, or unlistens immediately if it is not, so nothing is stranded on that path. That is only true of a late reply; Section 6.5 covers a reply that never arrives.

### 4.9 The microtask cost, and the one test helper that must change

**This is a measured cost of the chosen shape, not a surprise, and the implementer will meet it at step 6 of Section 8.**

`flushMount()` (`App.test.tsx:1061-1063`) is `for (let i = 0; i < 50; i += 1) await Promise.resolve()`. Four #1188 window tests open with an assertion taken before any timer is advanced: T1 (`:1110-1111`), T2 (`:1148-1149`), T3 (`:1173-1174`) and T6 (`:1212-1213`), each `await flushMount(); expect(callsFor("get_watcher_activity")).toHaveLength(1)`.

Wrapping all eight awaits adds microtask hops per step: `Promise.race` attaches a `then` (+1), `.finally()` is specified as a `then` whose handler returns `Promise.resolve(x).then(...)` (+2), and the `step` async frame adds its own. **Two independent models, written without sight of each other, both put the mount at 58 turns with the plan applied** (`dev-webpage-ui` measured 19 before and 58 after; the architect measured 25 before and 58 after, on Node 24.13). The baselines differ; the post-change figure does not, and it is the one that matters: **58 > 50, so T1, T2, T3 and T6 fail at their first assertion.**

**The fix, which `App.test.tsx:1054-1057` authorises in writing** ("if a future change outgrows the count, raise the count rather than reaching for `waitFor`"): raise the count at `:1061-1063` from 50 to **200**, and record why in its doc comment. 200 is ~3.4x the measured 58, the same "a margin, not a measurement" posture the existing comment takes, and 200 microtask turns cost microseconds. The per-call precondition assertions the comment relies on are unchanged, so an undercount would still fail loudly rather than pass vacuously.

**A cold-start implementer who sees T1 fail at `toHaveLength(1)` before touching a timer is looking at this and not at broken arming.** Raise the count; do not reach for `waitFor`, which `:1046-1060` explains cannot work under fake timers.

`flushMount()` is the **only** part of `App.test.tsx:976-1229` that changes. The four tests themselves, their assertions and their timer arithmetic are untouched.

### 4.10 Single-flighting this window's `get_settings`

**Arming the poll introduces an unbounded accumulation that must be closed in the same change.**

`runPollRound`'s `finally` calls `settingsStore.refresh()` at `:460`, which is `settingsStore.load().catch(...)` (`src/shared/stores/settings.ts:28-32`) reaching `invoke("get_settings")` with no deadline anywhere. Today a hung `get_settings` leaves exactly **one** pending `invoke`, because the mount never arms the poll. After this change the poll is armed, so the window would issue a new never-settling `invoke` every period, 6/min, for as long as it is open. Each retains a JS promise chain, a Tauri IPC callback registration and, on the native side, an independently spawned Rust task. That is precisely the call storm Section 3 rejects retries for, arriving by a different door.

**Decided: a single-flight guard local to this window, covering both the mount load and the poll refresh.**

```ts
/** At most one `get_settings` in flight from this window at a time.
 *
 *  #1196: arming the poll means `runPollRound` refreshes the settings every period, and
 *  nothing on that path is bounded (`stores/settings.ts:28-32` -> `invoke("get_settings")`,
 *  `transport-tauri.ts:23-26`). Without this, a `get_settings` that never answers collects
 *  one abandoned call per period for the life of the window. The flag is cleared by
 *  `finally`, so a call that eventually lands frees the next refresh; a call that never
 *  lands correctly stops this window from asking again. */
let settingsLoadInFlight = false;
const loadSettingsOnce = (): Promise<void> => {
  if (settingsLoadInFlight) return Promise.resolve();
  settingsLoadInFlight = true;
  return settingsStore.load().finally(() => {
    settingsLoadInFlight = false;
  });
};
```

Mount step #1 becomes `await step(loadSettingsOnce(), "load settings")`. `runPollRound`'s `:460` becomes:

```ts
void loadSettingsOnce().catch((err) =>
  console.error("[watchers] settings refresh:", err)
);
```

fired and not awaited, exactly as `settingsStore.refresh()` is today. On the happy path the flag is cleared within milliseconds, so every period still refreshes and the behaviour documented at `:457-459` is unchanged.

**The shared store is not modified.** `settingsStore` is used by the sidebar, the terminal and the main window; a global single-flight would change refresh semantics for consumers this issue has not analysed. The guard binds this window only.

**What it costs on the unhappy path, stated rather than left as a mechanism.** A `get_settings` that is lost *after* a successful startup latches the flag, so this window stops refreshing its settings for the rest of its life: the theme and the live agent labels freeze at their last successful values, and **nothing appears on screen**, because the startup notice is about startup and this hang is not. That is accepted, and Section 6.5 records it. It is a narrower cost than it first reads: while `get_settings` is genuinely not answering, issuing more of them buys no freshness at all, so the only case the latch actually costs anything is one permanently lost reply followed by a backend that recovers. Weighed against an unbounded accumulation for the life of the window, that trade is the right way round, and reopening the window is the recovery.

**`get_watcher_activity` accumulates the same way and is deliberately left alone.** That is #1188's already-accepted consequence, documented at `:145-147`, and re-deciding it is out of scope. Section 6.5 names it as an inherited residual so nobody reads Section 3's "no retry" as covering it.

## 5. Affected surfaces: exact files and symbols

### 5.1 `src/watchers/App.tsx` (the only production file)

| Anchor at `fd908941` | Change |
|---|---|
| after `:71` (`POLL_TIMEOUT_MESSAGE`) | add `MOUNT_TIMEOUT_MS`, `MOUNT_TIMEOUT_MESSAGE`, `STARTUP_DEGRADED_MESSAGE` with the doc comments in Section 4.7 |
| `:201` (`loadError` signal) | add the `startupDegraded` signal directly below it |
| near `:181` (the mount-scoped mutable state) | add `let settingsLoadInFlight = false;` and the `loadSettingsOnce` helper of Section 4.10 |
| `:380-390` | split into `loadSessions` (rejects) and `reloadSessions` (best-effort wrapper), per Section 4.5 |
| `:460` | `settingsStore.refresh()` becomes the `void loadSettingsOnce().catch(...)` of Section 4.10 |
| `:478` | insert `const budgetEndsAt = Date.now() + MOUNT_TIMEOUT_MS;` and the `step()` helper as the first statements of the `onMount` body, before the outer `try` |
| `:478-541` | open an inner `try` around the eight steps only, closed by the `finally` described in Section 4.2; the geometry block at `:532-541` stays inside the **outer** `try`, after the inner block |
| `:481-483` | becomes `await step(loadSettingsOnce(), "load settings");`, dropping the inline `.catch` |
| `:484` | `if (disposed) return;` unchanged |
| `:489` | becomes `await step(register(onWatcherMatches(...)), "subscribe to watcher matches");` |
| `:498` | becomes `await step(register(onWatchersScopeRequest(...)), "subscribe to scope requests");` |
| `:506` | becomes `await step(register(onSessionCreated(...)), "subscribe to session created");` |
| `:507` | becomes `await step(register(onSessionDestroyed(...)), "subscribe to session destroyed");` |
| `:508` | becomes `await step(register(onSessionRenamed(...)), "subscribe to session renamed");` |
| `:510` | becomes `await step(loadSessions(), "list sessions");` |
| `:511` | `if (disposed) return;` unchanged |
| `:518` | `const generationAtIssue = scopeEventGeneration;` unchanged |
| `:519-526` | the inline `try/catch` is replaced by `const pulled = await step(WindowAPI.getWatchersScope(), "pull the requested scope");` followed by the existing `if (disposed) return;` and the existing generation-guarded `setScopeSessionId(pulled)` |
| `:529-530` | moved verbatim into the `finally`, behind `if (!disposed)` |
| `:532-541` | unchanged, and still after the arming |
| `:542-550` | unchanged |
| after `:762` | insert the notice `<Show>` block, between the `loadError` banner and the `truncated` banner |

The notice block, reusing the existing `.watchers-banner` class so no CSS changes:

```tsx
{/* Startup, not the last round: this one is written once and never cleared, because the
    window cannot tell which of the losses it is reporting will repair themselves. */}
<Show when={startupDegraded()}>
  <div class="watchers-banner" data-ac-testid="watchers.startupDegraded">
    {STARTUP_DEGRADED_MESSAGE}
  </div>
</Show>
```

Placement is a pure insertion between two existing siblings, so no existing `data-ac-testid` query changes meaning and no existing test's DOM assumptions move.

### 5.2 `src/watchers/App.test.tsx`

- `:812-837` rewritten to the new contract (Section 9.1, test M5), including its doc comment, which currently states the invariant being reversed.
- `:1061-1063`, the `flushMount()` turn count, raised from 50 to 200 with its doc comment amended (Section 4.9). **This is the only change inside `:976-1229`.**
- A new `describe("the watcher window mount chain (#1196)")` suite appended (tests M1 to M4 and M6 to M8).
- `:3-11` import list gains `MOUNT_TIMEOUT_MS` and `STARTUP_DEGRADED_MESSAGE`. The list is case-insensitively alphabetical (`logicalGeometry, POLL_FOCUSED_MS, POLL_TIMEOUT_MESSAGE, ...`), so `MOUNT_TIMEOUT_MS` goes after `logicalGeometry` and `STARTUP_DEGRADED_MESSAGE` after `registerAll`.
- `:844-863` is **not** modified and must keep passing.
- Everything else in `:976-1229` is **not** modified and must keep passing.

### 5.3 Files deliberately not touched

`src/watchers/activity.ts`, `src/watchers/styles/watchers.css`, `src/watchers/components/WatchersTitlebar.tsx`, `src/shared/ipc.ts`, `src/shared/transport-ws.ts`, `src/shared/transport-tauri.ts`, `src/shared/platform.ts`, `src/shared/stores/settings.ts`, `src/main.tsx`, `src/shared/testing/*`, and the whole of `src-tauri/`.

## 6. Required behaviour, edge cases and behaviour on failure

### 6.1 Invariants

1. **The window always arms.** For every possible outcome of every one of the eight startup awaits, except the window being closed, `setScopeSettled(true)` and `schedulePoll()` run, and they run before `trackGeometry()`.
2. **The window arms within one budget.** From the first statement of `onMount`, the time to arming is at most `MOUNT_TIMEOUT_MS` plus the settle overhead of the remaining zero-budget steps, **under a responsive event loop**, regardless of how many awaits stall.
3. **Every startup step is attempted.** A failure at step *n* does not skip steps *n+1..8*. Steps that inherit an exhausted budget still issue their IPC call.
4. **Teardown ends the mount and arms nothing.** After `onCleanup` has run, no poll timer is armed, no fetch is issued, and no notice is painted.
5. **The notice is persistent.** Once raised it stays for the life of the window. It is independent of `loadError`, and a successful poll clears `loadError` without touching it.
6. **At most one `get_settings` is in flight from this window** at any moment.
7. **The happy path is identical in observable behaviour and in the call sequence to the backend.** Nothing new is rendered and no timer outlives the mount (`withDeadline` clears its timer on both paths, `:166-168`). It is not identical in cost: the mount takes about 33 additional microtask turns, which is Section 4.9's subject.

### 6.2 What each lost startup step actually costs

The five subscriptions are **not** interchangeable and must not be described as one row.

| Lost step | Consequence, precisely |
|---|---|
| #1 `get_settings` | wrong theme and frozen agent labels until a poll period in which `loadSettingsOnce` succeeds; `:240-245` and `:230-235` both read the store |
| #2 `watcher_matches` | no live match rows; the poll still repaints the table every period, so matches appear late rather than never |
| #3 `watchers_scope_request` | later re-scope requests from the terminal are lost; the scope `<select>` at `:655-666` still shows the live scope, so the state is visible and manually recoverable |
| #4 `session_created` | **a session created after this point never enters `sessions` at all.** `scopeIds()` excludes it (`:220-224`), the match handler rejects its batches (`:490-497`), and the poll cannot discover it because the session list is not polled: `loadSessions` runs only from the mount and from these three listeners |
| #5 `session_destroyed` | a destroyed session stays in the list and in "All agents" scope until some other surviving session event triggers a reload |
| #6 `session_renamed` | stale session names in the `<select>` and in frozen rows, same recovery condition as #5 |
| #7 `list_sessions` | an empty session list; under "All agents" that means an empty scope and therefore no rows at all until a surviving session listener fires |
| #8 `get_watchers_scope` | the query-parameter scope stands, which `:514-517` already documents as the deliberate outcome of a failed pull |

These are persistent partial-initialization states, not delays. That is why the notice is persistent and why it says to reopen.

**Some of them can repair themselves and the window cannot tell which.** A step abandoned at the deadline is not cancelled, so a late `loadSessions()` reply still calls `setSessions` if the window is alive and the stale guard passes, and a late `register` still installs its listener. The notice is therefore a statement about startup, not a claim that the losses are permanent. Clearing it would require knowing which losses healed, which the window does not know.

### 6.3 Zero remaining budget: the outcome is timing-dependent, and that is accepted

Once the budget is exhausted, `Math.max(0, ...)` yields `0` and `withDeadline` arms a zero-delay timer. **This does not mean the step fails.** A `Promise.race` between work whose continuation is already queued and a zero-delay timer is won by the work, because a microtask runs before a macrotask; work that is still waiting on anything asynchronous loses. `dev-rust-grinch` confirmed both directions with a Node 24.13 probe: an already-fulfilled promise resolves, a 5 ms async reply is beaten by the deadline.

For step #8 the winner decides whether the pulled scope or the query-parameter scope stands. **This is deliberately not pinned by a test.** Both outcomes are states the design already treats as valid and already reaches by other routes: `:514-517` documents the query-parameter scope as the correct result of a failed pull, and a successful pull is the normal result. The race cannot produce a state that is otherwise unreachable, so pinning it would freeze an implementation detail rather than a contract. It is named here so that it is documented rather than hidden.

### 6.4 The "All agents" residual, stated honestly

If `list_sessions` is lost at mount **and** the scope is "All agents", `scopeIds()` is `[]`, `refresh()` resolves `Promise.all([])`, and the view is `"warming"`: the window paints `Waiting for the first sample...`. Four things make this acceptable rather than a half-fix:

1. **The notice is up**, for a rejection as well as for a timeout. That is what the `loadSessions` split of Section 4.5 exists to guarantee, and M6 is the test.
2. The poll chain is alive, so the window is not dead.
3. It is not the common case. `main.tsx:51` passes `?sessionId=` and `StatusBar.tsx:63-67` always sends one, so a freshly opened window is normally scoped to a single session; the residual mainly bites after the user selects "All agents" in the `<select>` at `:655-666`.
4. It is not a new failure: a *rejecting* `list_sessions` produces the same empty state today, silently.

**The self-heal is conditional and the plan does not claim otherwise.** Recovery on a session event requires that the corresponding listener registered successfully. In the canonical failure this fix exists for, a transport that has stopped answering, step #1 consumes the budget and steps #2 to #6 are abandoned, so those listeners may never exist and nothing can heal until the transport recovers on its own. Reopening the window is the recovery gesture the notice names, and it is the only unconditional one.

### 6.5 Inherited and accepted residuals

- **`get_watcher_activity` accumulates one abandoned `invoke` per poll period while it is hung.** This is #1188's accepted consequence (`:145-147`), not something this change introduces, and it is deliberately not re-decided here. Section 3's "no retry of a failed startup step" is about the mount chain and must not be read as covering the poll.
- **A Tauri subscription whose reply is truly lost cannot be cleaned up.** `node_modules/@tauri-apps/api/event.js:74-81` builds the `unlisten` closure only inside the invoke's `.then`, so if that reply never arrives, `register` never receives an unlisten and neither its `disposed` branch nor `onCleanup` can remove it, while the backend listener mapping may already exist. Reported by `dev-rust-grinch` as a **conditional inference** on "side effect completed, reply truly lost", not as an observed Tauri behaviour, and it is a property of the Tauri event API rather than of this change: the same window today leaks the same way if a `listen` reply is lost mid-mount. Recorded, not closed. Also noted there: `UnlistenFn` is typed `() => void` (`src/shared/transport.ts:19`) while Tauri's real unlisten is async (`event.js:38-45`), so cleanup invokes without awaiting backend removal. Unchanged by this plan.
- **A `get_settings` lost after startup freezes this window's settings silently.** The single-flight flag of Section 4.10 latches on a call that never settles, so the theme and the live agent labels stay at their last successful values for the life of the window, with nothing on screen: the startup notice does not cover a post-startup hang, and by construction it must not, since startup succeeded. Accepted for the reason in Section 4.10, namely that more calls to a command that is not answering buy no freshness, so the only case this costs anything is a permanently lost reply followed by a recovering backend. Reopening the window is the recovery.
- **`Date.now()` is wall-clock.** A machine suspend between two mount steps can expire the budget on an otherwise healthy start and paint the notice. The mount is a sub-second operation, the outcome is cosmetic plus a possible loss of late steps, and there is no retry storm. Accepted rather than switching to `performance.now()`, which would need the same change in the test harness's clock assumptions for no proportionate gain.

## 7. Compatibility and security

- **No IPC surface change.** No command is added, removed or renamed; no payload shape changes; no Rust file is touched; `src/shared/types.ts` is unchanged.
- **No persistence change.** No settings key, no TOML shape, no migration.
- **No new dependency.**
- **No shared-module change.** `src/shared/stores/settings.ts` keeps its current semantics for every other window (Section 4.10).
- **Windows/ConPTY:** nothing on this path touches the PTY, spawning or shell wrapping.
- **Security:** the change adds no new input handling and no new privileged call. The one new user-visible string is a fixed literal; no error text, path or identifier from the backend is rendered by it, so nothing untrusted reaches the DOM through this change. The underlying error goes to `console.error`, exactly as five existing sites in this file already do.
- **Behavioural compatibility:** one documented invariant is deliberately reversed (a failed subscribe no longer ends the mount). It is user-visible only in the failure case, where the old behaviour was a dead window.

## 8. Implementation order

1. Add the three constants after `:71` with their doc comments.
2. Add the `startupDegraded` signal after `:201`.
3. Add the notice `<Show>` block after `:762`. Checkpoint: typecheck clean, every existing test still passes, because nothing yet sets the signal.
4. Add `settingsLoadInFlight` and `loadSettingsOnce` (Section 4.10) and switch `:460` to it. Checkpoint: full suite still green.
5. Split `reloadSessions` into `loadSessions` + `reloadSessions` (Section 4.5), leaving `:510` calling `reloadSessions()` for now. Checkpoint: full suite still green, because the split is behaviour-preserving for every existing caller.
6. Add `budgetEndsAt` and `step()` at the top of `onMount`.
7. Convert the eight awaits to `step()` calls, one at a time, pointing #1 at `loadSettingsOnce()` and #7 at `loadSessions()`, keeping the three `if (disposed) return;` checks and the generation guard exactly where they are.
8. Introduce the inner `try/finally` and move `:529-530` into it.
9. **Expect T1, T2, T3 and T6 to fail here at their first assertion.** That is Section 4.9, not broken arming. Raise `flushMount()`'s count at `:1061-1063` from 50 to 200 and amend its doc comment. Record the count you actually needed if 200 is not enough; do not reach for `waitFor`.
10. Rewrite `App.test.tsx:812-837` to the new contract (test M5).
11. Add the new `#1196` suite (M1 to M4 and M6 to M8). Every one of them names its transport base and its render helper explicitly in Section 9.1; use the named ones, because M6's discrimination depends on its base and M8's on its being the happy path.
12. Full suite green, including the untouched `:844-863` and the rest of the #1188 suites.

## 9. Tests and acceptance criteria

### 9.1 The tests

All window tests go in a new `describe("the watcher window mount chain (#1196)")` and reuse the #1188 suite's harness: `installBrowserDomStubs()` + `resetUiStoresForTests()` in `beforeEach`, the `flushMount()` microtask driver (`:1061-1063`, now 200 turns), and `renderWithFakeClock` (`:1083-1087`), which installs fake timers **before** the render because the mount arms its own timers. `waitFor` must not be used under fake timers, for the reason given at `:1046-1060`. `initialSessionId="s1"` is mandatory so each round is one call. Every test restores its own mocks and timers in a `finally`, as `:1136-1140` does.

Add one local helper mirroring `errorBanner` at `:1065-1066`:

```ts
const startupBanner = (root: HTMLElement): HTMLElement | null =>
  root.querySelector<HTMLElement>('[data-ac-testid="watchers.startupDegraded"]');
```

**Draining the zero-length deadlines.** When step #1 consumes the budget, steps #2 to #8 each arm a `setTimeout(..., 0)`, created only after the previous step's rejection has propagated. Whether a single `advanceTimersByTimeAsync(0)` drains a chain of timers armed *during* the same tick is a property of the vendored fake-timer implementation that could not be verified in this repo (Section 11.3). **Every test below therefore drains with a bounded loop**, which costs nothing if one tick already suffices:

```ts
const drainZeroTimers = async (): Promise<void> => {
  for (let i = 0; i < 10; i += 1) await vi.advanceTimersByTimeAsync(0);
};
```

**M1 - a startup call that never settles still arms the poll, and does not accumulate calls.**
`transportWith(snapshot())`, then `fake.onInvoke("get_settings", () => new Promise(() => {}))` so step #1 hangs.
1. `await flushMount()`; assert `fake.callsFor("get_watcher_activity")` has length 0 and `startupBanner` is null. The mount is stuck at step #1, which is today's shipped behaviour.
2. `await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS - 1)`; assert still length 0. This pins that the budget is not shorter than specified, and it is what catches a per-await implementation using `MOUNT_TIMEOUT_MS / 8`.
3. `await vi.advanceTimersByTimeAsync(1)`, then `await drainZeroTimers()`, then `await flushMount()`.
4. Assert `startupBanner(rendered.root)?.textContent` is exactly `STARTUP_DEGRADED_MESSAGE`.
5. Assert `fake.callsFor("get_watcher_activity")` has length 1, which is the scope effect's first fetch, proving `scopeSettled` was set.
6. Assert `[data-ac-testid="watchers.empty.warming"]` is null and `[data-ac-testid="watchers.empty.unconfigured"]` is truthy, proving real data reached the view rather than the window merely looking alive. Derived from `snapshot()`'s defaults at `:28-40` (`warmedUp: true`, `activeWatchers: []`, `matches: []`).
7. `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS)`; assert `get_watcher_activity` length 2, proving the chain survives.
8. `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS * 3)`; assert `get_watcher_activity` grew **and** `expect(fake.callsFor("get_settings")).toHaveLength(1)`. This is the single-flight pin of Section 4.10: without it the count grows once per period.

**M2 - the chain continues past a hung step instead of jumping to the end.**
Hang step #6 by making `fake.listen` return a never-settling promise for `session_renamed` only, keeping the real `listen` for every other event (the same override shape as `:819-823`, with a pending promise instead of a rejection).
1. `await flushMount()`; assert `fake.callsFor("get_watchers_scope")` has length 0.
2. Advance the budget and drain as in M1.
3. Assert `fake.callsFor("get_watchers_scope")` has length 1. Step #8 was still **issued**, which is true whether or not its zero-budget race is won by the work (Section 6.3), so the assertion is stable. It deliberately does not assert which side won.
4. Assert the notice is up and `fake.callsFor("get_watcher_activity")` has length 1.

**M3 - the budget is shared, not per-await.**
Hang **two** steps: `get_settings` (#1) and `list_sessions` (#7).
1. `await flushMount()`; assert `get_watcher_activity` length 0.
2. `await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS)`, `await drainZeroTimers()`, `await flushMount()`.
3. Assert `get_watcher_activity` has length 1 and the notice is up. Under a per-await design at `MOUNT_TIMEOUT_MS` each this needs a second full budget and the assertion fails.

**M1 step 2 and M3 are a pair and neither works alone.** M3 catches a per-await implementation at 10 000 ms each; M1 step 2 catches one at `10_000 / 8 = 1_250` ms each, which M3 alone would let through. A future edit must not drop either.

**M4 - the constant's invariants.**
```ts
expect(MOUNT_TIMEOUT_MS).toBeGreaterThan(POLL_TIMEOUT_MS);        // strict: the floor
expect(MOUNT_TIMEOUT_MS).toBeLessThanOrEqual(POLL_FOCUSED_MS);    // non-strict: the ceiling
```
with Section 4.3's rationale as its comment, including the sentence on why `App.test.tsx:1015`'s strictness argument does not transfer to the mount. Fixation, not regression: expected to pass against any correct choice of the constant. Its job is to stop a later edit from letting the budget drift below one round's allowance or past one poll period.

**M5 - rewrite of `App.test.tsx:812-837`.**
Keeps its setup (step #6 rejects with `transport closed mid-subscribe`). New name: `"keeps starting up and reports it when a subscribe fails"`. New doc comment stating the new contract. Runs without fake timers, as it does today, because the rejection is immediate.
1. `await waitFor(() => expect(startupBanner(rendered.root)).toBeTruthy())`. **Do not copy the existing wait at `:827-831`**: it waits on `watchers.error`, which under the new contract never appears, so a verbatim copy times out after 1 s.
2. Assert the notice text is exactly `STARTUP_DEGRADED_MESSAGE`.
3. Assert `fake.callsFor("get_watcher_activity")` is **length 1**, replacing the old `toHaveLength(0)`. This is the assertion that inverts.
4. A `const errorSpy = vi.spyOn(console, "error")` recorded a call whose arguments include the `transport closed mid-subscribe` error, preserving the original test's real value, which was that a failed setup stays diagnosable. **`errorSpy.mockRestore()` in the test's `finally`.** The enclosing `describe` at `:100-864` has no `vi.restoreAllMocks()` in its `afterEach` (`:108-113`) and `vitest.config.ts:9-13` sets neither `restoreMocks` nor `clearMocks`, so an unrestored spy leaks into every later test in the file.

**M6 - a REJECTING `list_sessions` raises the notice.** The test the `loadSessions` split exists for.

Setup, in full, because this one departs from the shared harness and **the departure is exactly where it can be got wrong**:

```ts
const fake = transportWith(snapshot());
fake.reject("list_sessions", "the session list is gone");
const rendered = renderWithFakeTransport(() => <WatchersApp />, fake);
```

- **`transportWith(snapshot())` is mandatory as the base.** With a bare `new FakeTransport()` (the shape at `App.test.tsx:612`), `get_settings` and `get_watchers_scope` have no handler and `FakeTransport.invoke` throws `Unhandled fake transport invoke:` (`fake-transport.ts:53-54`). Steps #1 and #8 would then fail on their own, the notice would appear no matter what step #7 did, and **M6 would pass against the `step(reloadSessions(), ...)` implementation it exists to reject.** With `transportWith`, step #1 resolves (`:76`), steps #2 to #6 register, step #8 resolves to `null` (`:80`), so **step #7 is the only possible source of the notice.** Do not change this base.
- **`renderWithFakeTransport` is mandatory as the helper**, not `renderWithFakeClock`: the latter hardcodes `initialSessionId="s1"` at `:1086`, and M6 must render **without** one so the scope is "All agents".
- **No fake timers**, because the rejection is immediate. M6 relies on the preceding tests in this suite having restored real timers in their own `finally`, which Section 9.1 requires of every test here.

1. `await waitFor(() => expect(startupBanner(rendered.root)).toBeTruthy())`.
2. Assert the notice text is exactly `STARTUP_DEGRADED_MESSAGE`.
3. Assert the window is not silent about it: `[data-ac-testid="watchers.empty.warming"]` may legitimately be present (Section 6.4), so assert the **notice** rather than the absence of the warming state.

Against an implementation that wraps `reloadSessions()` instead of `loadSessions()`, the wrapper's catch fulfils, `step()` sees success, `startupDegraded` is never set and the `waitFor` at assertion 1 times out. Fails, correctly, and for the right reason.

**M7 - an abandoned subscription that answers late does not strand a listener.**

Setup: `transportWith(snapshot())` as the base, with `session_created` (#4) served by a deferred `listen` that the test resolves by hand, and `renderWithFakeClock(fake)` as the helper, because advancing the budget needs fake timers.

1. `await flushMount()`, then advance and drain the budget as in M1, so step #4 is abandoned and the mount arms.
2. `rendered.cleanup()`.
3. Resolve the deferred with an unlisten spy, then `await flushMount()`.
4. Assert the spy was called **exactly once**: `register`'s continuation lands with `disposed` true, so it unlistens and throws the sentinel, which the race's still-attached handler absorbs (Section 4.8).
5. Restore timers and mocks in the `finally`.

This pins the half of the claim that is true (a **late** reply is cleaned up) and deliberately does not claim anything about a reply that never arrives, which is Section 6.5's recorded residual.

**M8 - a healthy window still refreshes the settings every period.**

Setup: `transportWith(snapshot())` and `renderWithFakeClock(fake)`. Nothing is hung; this is the happy path.

1. `await flushMount()`; assert `fake.callsFor("get_settings")` has length **1**, the mount's own step #1.
2. `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS)`, `await flushMount()`; assert length **2**.
3. `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS)`, `await flushMount()`; assert length **3**.
4. Assert `startupBanner(rendered.root)` is null, pinning that a healthy start paints nothing new (Section 6.1.7).

**M8 is a fixation test and passes against `fd908941`. That is expected and it is excluded from AC1.** Its discrimination target is not the defect but a wrong *fix* for it: an implementer who **deletes** `:460` instead of routing it through `loadSettingsOnce` passes every other test in this plan while silently losing the behaviour documented at `App.tsx:457-459`, where a watcher saved in the Settings modal appears without reopening the window. Against that implementation the count stays at 1 and step 2 fails.

**M1 step 8 and M8 are a pair and neither works alone**, the same shape as the M1 step 2 / M3 pair stated above. M1 step 8 proves the count does not grow while `get_settings` is hung, which a deleted `:460` also satisfies; M8 proves it does grow when `get_settings` is healthy, which an unguarded `settingsStore.refresh()` also satisfies. Only together do they pin "guarded, and still there". A future edit must not drop either.

### 9.2 Acceptance criteria

Objective, and none of AC1's tests can pass against `fd908941`:

1. **AC1.** M1, M2, M3, M5 and M6 fail on `fd908941` and pass after the change. All five import `MOUNT_TIMEOUT_MS` or `STARTUP_DEGRADED_MESSAGE`, so against `fd908941` they fail to link rather than to behave. Run all five against a scratch checkout with **only the three constants added** and confirm they then fail on behaviour. An import error is not proof of discrimination.
2. **AC2.** `App.test.tsx:844-863` passes unmodified, and `:976-1229` passes with `flushMount()`'s turn count at `:1061-1063` as its **only** modification.
3. **AC3.** The full frontend suite is green, and the repo's typecheck/build is clean.
4. **AC4.** `grep -rn "withDeadline" src/` returns the definition, `App.tsx:349`, exactly one new use inside `step()`, the doc reference at `App.tsx:442`, and the test references. Any other production call site means the change went wider than this plan.
5. **AC5.** Every one of the eight positions goes through the bound, which AC4 does not prove. Check both:
   - `grep -c "await step(" src/watchers/App.tsx` returns exactly **8**.
   - Reading the inner `try` block of `onMount` end to end, **every `await` token in it is part of an `await step(` expression**. There must be no other `await` between the inner `try` and its `finally`. This is the mechanical form of invariant (i) in Section 4.2, and it is the check that catches a ninth await added later without a wrapper.
6. **AC6.** `git diff --stat fd908941` touches exactly `src/watchers/App.tsx` and `src/watchers/App.test.tsx`. **This plan file will not appear there**: `.gitignore:11` ignores `plans/`, and the existing plan files are tracked only because they were force-added. Committing this one therefore needs `git add -f plans/1196-watcher-mount-poll-chain.md`, and whether it is committed at all is the coordinator's call, not the implementer's.
7. **AC7.** Manual, on the desktop build: open the watcher window normally and confirm no notice appears, the table populates and the poll ticks. This is the only check that exercises `TauriTransport`, which no jsdom test reaches.

## 10. Why the rejected shapes are rejected

- **Deadline the eight awaits and change nothing else.** Rejected: Section 2.5 traces it to a window with a banner that still never polls.
- **Per-await deadlines.** Rejected: multiplies to a worst case of eight deadlines on a chain where a stalled backend makes the multiplication the expected case, not the corner case.
- **Change `withDeadline` to take a cumulative budget.** Rejected: it forces a signature change and a regression audit of `:349` for no gain, because the caller can express a shared budget with `Math.max(0, budgetEndsAt - Date.now())` against the unchanged helper.
- **Arm the poll before the chain instead of after it.** Rejected: `scopeSettled` gates the first fetch precisely so that a fetch is not issued for a session the window is about to leave (`:414-415`), and arming early reopens that.
- **A watchdog timer that arms the poll if the mount has not.** Rejected: a second, independent arming path can double-arm the chain and it duplicates a guarantee that the bound-plus-`finally` pair gives for free.
- **Retry failed startup steps.** Rejected: it points a retry loop at a backend that is already not answering, and reopening the window is an existing, cheap, user-controlled retry.
- **Reuse `loadError` for the startup notice.** Rejected: it is cleared at `:433` and `:358`, so the message would vanish within about one poll period of the window arming.
- **Wrap `reloadSessions()` at the mount instead of splitting it.** Rejected: its internal catch fulfils the promise, so `step()` cannot see a rejection and the notice would be absent in exactly the case that reproduces the issue's symptom (Section 4.5).
- **Put the settings single-flight in `src/shared/stores/settings.ts`.** Rejected: the store is shared with three other windows whose refresh semantics this issue has not analysed. The guard binds this window only (Section 4.10).
- **Gate the budget on `isTauri`.** Rejected: bounds compose as a minimum so there is nothing to avoid, #1188's `withDeadline` at `:349` is already transport-agnostic, the shipped router cannot put `WatchersApp` on `WsTransport` at all (`main.tsx:35-36` before `:47`), and a transport branch inside a window component would be a new pattern this change is not the place to introduce. **Note for anyone re-opening this**: an `isTauri` branch would *not* be untestable. `vi.mock("../../shared/platform", ...)` is an established pattern in this repo, used by nine test files including `StatusBar.watchers.test.tsx:7`, whose comment documents a getter that exposes both sides of the gate in one file. An earlier draft of this plan claimed otherwise and was wrong.
- **Switch `MOUNT_TIMEOUT_MS` to 8 000 for a strict ceiling.** Rejected: the strictness argument at `App.test.tsx:1011-1014` is about a chained cadence the mount does not have (Section 4.3), and tightening the budget raises the risk of the failure that costs more, which is bounding out a healthy-but-slow start.

## 11. What could not be verified

### 11.1 The backend: no deadlock was established, and the earlier claim of one was wrong

An earlier draft of this plan described `list_sessions` as taking `session_mgr.read().await` and then `settings.read().await` in a nested pattern and called a lock-order deadlock plausible. **That is false, and it is corrected here.** Verified at `fd908941`:

- `src-tauri/src/commands/session.rs:4265` is `let mut infos = { session_mgr.read().await.list_sessions().await };`. The manager guard is **block-scoped and dropped** before `let cfg = settings.read().await;` at `:4268`. The two guards are never held together.
- A repo-wide search for `(session_mgr|session_manager)\.(write|blocking_write)\(\)` in `src-tauri/src/` returns **no matches**, so the manager lock has no opposite writer edge in production source.
- `get_settings` (`config.rs:473-475`) reaches `settings_snapshot_helper`, which takes an unconditional **write** inside a block at `config.rs:415` (its own comment: "Reconciliation transaction under the write guard; no lock across await"), drops it, then takes a read at `:468`.
- `get_watchers_scope` (`window.rs:868-873`) takes only its own private mutex.

**No cycle exists among these locks.** The frontend hardening does not depend on one.

A different and real mechanism was found by `dev-rust-grinch` and re-verified here: while holding the settings read guard at `session.rs:4268`, the loop at `:4271` calls `compute_profile_outdated`, which can reach `read_replica_profile_content_hash` (`session.rs:2684-2687`) and from there `read_json_object`'s synchronous `std::fs::read_to_string` (`src-tauri/src/config/coding_agent_profiles.rs:45-46`). UNC working directories are supported. A slow or disconnected UNC path therefore holds a settings **reader** while blocking, which a pending `get_settings` **writer** at `config.rs:415` must wait behind. That is **reachable blocking I/O and lock contention, not a proven deadlock and not a reproduced incident.** Nothing in this plan depends on it either: the fix is correct for any cause of a lost or late reply, including a stalled `TauriTransport.init()` dynamic import, which would stall every call the window makes rather than one.

### 11.2 The microtask figures

The 25/58 and 19/58 counts in Section 4.9 come from two independent models of the promise topology, not from running the suite. The **post-change figure of 58 was reproduced identically by both**, and the direction is not in doubt, but the exact numbers are inferences. Step 9 of Section 8 settles it: run `App.test.tsx` and read the result.

### 11.3 The fake-timer drain

Whether `vi.advanceTimersByTimeAsync(0)` re-scans for timers armed during the same tick could not be verified: Vitest 4.1.5 vendors its fake timers and `@sinonjs/fake-timers` is not installed standalone. Section 9.1's `drainZeroTimers()` loop makes every test that depends on it robust either way, at zero cost.

### 11.4 Everything through `TauriTransport`

No jsdom test reaches it. AC7 is the only closer, and it is manual.

### 11.5 The Tauri listener residual

Section 6.5's truly-lost-reply case is `dev-rust-grinch`'s conditional inference from the Tauri 2.10.3 sources, not an observed behaviour. No desktop reproduction of a lost reply was attempted by anyone in this pipeline.

### 11.6 Measurement of the eight startup commands

None exists. Section 4.3 says so explicitly and rests its headroom argument on a pessimistic assumption rather than on the 13 ms figure, which belongs to a different command.

### 11.7 The gate

`codebase-memory` was bypassed under explicit authorization (issue #1205, `base_sha` returns empty), by the architect and by both enrichers. Every coordinate here comes from `git` plus direct file reads at `fd908941`. The gate was not repaired and no workaround was invented.

## 12. Step 7 resolution record

Round 1 of a maximum of 3. `dev-webpage-ui` returned NOT READY (1 blocking, 7 should-fix, 4 optional); `dev-rust-grinch` returned FAIL (2 blocking, 6 should-fix, 1 optional). Neither edited the plan. Both stated that the core shape (shared budget + `step()` + arming in a `finally`) survived their attacks, and both cleared the `toHaveLength(0)`-to-`(1)` reversal as a legitimate contract change rather than a regression.

**Accepted in full.** Every one of the following changed the plan body; nothing was accepted in name only.

| Finding | Where it landed |
|---|---|
| dev B1 - the mount exceeds `flushMount()`'s 50 turns | new §4.9, §5.2, §6.1.7, AC2, step 9 of §8. **Independently re-measured by the architect: 25 before, 58 after, on Node 24.13. Both models agree on 58, which is the load-bearing number.** Count raised to 200 |
| dev S3(a) / grinch #1 - a *rejecting* `list_sessions` raises no notice | `loadSessions`/`reloadSessions` split in §4.5, §5.1, test M6, §6.4 point 1 |
| dev S4 / grinch #2 - arming the poll retries `get_settings` unboundedly | new §4.10 (`loadSettingsOnce` single-flight), §5.1, §6.1.6, M1 step 8, §3 out-of-scope reworded |
| dev S2 - "no test mocks `src/shared/platform`" is false | removed from §4.6 and corrected in §10 with the counter-evidence. Nine files mock it; the architect's original grep was too narrow |
| dev S5 / grinch #6 - the `<=` invariant cited the wrong precedent | §4.3 now argues the ceiling on its own terms and states why `App.test.tsx:1015`'s strictness does not transfer; `:1018` is no longer cited; M4 gains a strict floor assertion |
| grinch #6 - "strictly smaller per call" is false | deleted. §4.3 now says the opposite explicitly: step #1 can consume nearly the whole budget |
| grinch #6 - the 13 ms datum is for a different command | §4.3 and §11.6 now say no measurement exists for the eight startup commands |
| grinch #6 - `setTimeout` is a minimum, `Date.now()` is wall-clock | §4.3, §4.7, §6.1.2 reworded to "service target under a responsive event loop"; §6.5 records the wall-clock consequence |
| grinch #3 - the Rust lock inversion does not exist | §11.1 rewritten and the false claim retracted. Re-verified by the architect: the guard at `session.rs:4265` is block-scoped, and no manager writer exists in production source |
| grinch #5 - the `finally` "structural guarantee" is logically false | §4.2 now states the two-part invariant; AC5 added to check part (i) mechanically |
| grinch #4 - "no listener is stranded" over-generalises | §4.8 narrowed to a late reply; §6.5 records the truly-lost-reply residual; test M7 pins the half that is true |
| grinch #7 - zero budget is timing-dependent in production | new §6.3, which accepts it with reasons and says why it is deliberately not pinned by a test |
| grinch #8 - the consequence table understates four subscriptions | new §6.2 with one row per lost step; the false "none repair themselves" claim replaced with "the window cannot tell which" |
| grinch #9 - the shipped router cannot put `WatchersApp` on WS | §2.8 and §4.6 reframed; the WS analysis is retained as conditional rather than as a live constraint |
| dev S6 - AC1 omitted M5 | AC1 now covers M1, M2, M3, M5 and M6 |
| dev S7 - the fake-timer drain is unverified | `drainZeroTimers()` in §9.1, assumption recorded in §11.3 |
| dev S8 - M5's `console.error` spy leaks | M5 assertion 4 now mandates `mockRestore()` in a `finally`, with the reason |
| dev O1 to O5 | §6.1.7 wording, §4.2's `isTauri` note, M5's wait condition, §5.2's import placement, §6.5's wall-clock line |

**Rejected, with reasons, so they are not rediscovered:**

- **Setting `MOUNT_TIMEOUT_MS = 8_000` to make the ceiling strict** (dev S5's first option). The strictness argument at `App.test.tsx:1011-1014` is about reporting latency against a chained cadence; the mount has no cadence behind it, so nothing is due at t=10 000 for the budget to collide with (§4.3). Tightening also raises the probability of the costlier failure, which is bounding out a healthy-but-slow start. The citation defect dev correctly identified is fixed; the constant is not.
- **Pinning the zero-budget race outcome with a test** (grinch #7's second option). Both outcomes are states the design already reaches by other routes and already treats as valid, so a test would freeze an implementation detail rather than a contract (§6.3). The boundary is documented instead of hidden.
- **Moving the single-flight into `src/shared/stores/settings.ts`** (grinch #2 raised the store as an option). Three other windows consume that store and this issue has not analysed their refresh semantics (§4.10, §10).
- **Adding a test for the truly-lost-reply Tauri listener** (grinch #4's second option). It is not testable through `FakeTransport` without asserting a behaviour nobody has observed; grinch said as much themselves. M7 pins the late-reply half, and §6.5 records the rest as a residual of the Tauri event API rather than of this change.

**Not re-litigated**, because both enrichers verified them independently: the raw arithmetic; the `finally` boundary including `App.test.tsx:844-863` surviving unmodified; the `loadError` clear sites; the absence of an i18n layer; `.watchers-banner` existing; the new tests genuinely failing against `fd908941`; M1+M3 catching both per-await variants as a pair; and the WS path being safe under a 10 s budget were it ever reachable.

### 12.1 Recertification round: the two shapes round 1 introduced

The `loadSessions`/`reloadSessions` split (§4.5) and the `loadSettingsOnce` single-flight (§4.10) were written during Step 7 round 1 and so were never seen by an enricher. `dev-webpage-ui` re-checked exactly those two and returned **both SOUND**, verifying: the stale/disposed guard placement across all four cases of the split, including that a naive rethrow would have started logging stale rejections that are swallowed today; that the caller set is closed at four sites with no caller newly exposed to a rejection; that `let listed: Session[]` passes `tsc --noEmit --strict` (exit 0) on definite assignment; and, for the single-flight, that it cannot deadlock, cannot latch by accident (`load()` is `async`, so it cannot throw synchronously and orphan the flag), self-repairs on a late landing, cannot be entered by the mount through its early-return path, and still propagates a rejection to the notice.

**Neither design changed.** Two test-coverage gaps were found and closed, both of the "green test that proves nothing" family:

| Gap | Resolution |
|---|---|
| M6's transport base and render helper were unstated, and a bare `new FakeTransport()` would have made steps #1 and #8 fail on their own, so the notice would appear regardless of step #7 and **M6 would pass against the implementation it exists to reject** | §9.1's M6 now specifies `transportWith(snapshot())` + `renderWithFakeTransport(() => <WatchersApp />, fake)` in a code block, with the reason each is mandatory and an instruction not to change the base. M6 is the sole pin on one of round 1's two convergent blocking findings |
| Nothing distinguished "single-flighted" from "the `:460` refresh was deleted": M1 step 8 only pins that the count does not grow | **New test M8**, a happy-path case asserting the count grows once per period, plus the statement that M1 step 8 and M8 are a pair. M8 is fixation, passes against `fd908941`, and is excluded from AC1 |

Two consequential edits made in the same pass, both disclosed to the coordinator rather than folded in silently:

- **M7 carried the identical defect** to M6's: no transport base and no render helper, plus a "unit-level, no window" phrase contradicting its own description of a rendered mount. Fixed the same way. Leaving the identical hole in the adjacent test while closing it in M6 would have been indefensible in a document that is the sole specification.
- **§4.10 stated the latch only as a mechanism**, so §4.10 and §6.5 now state its user-visible cost: a `get_settings` lost *after* startup freezes this window's theme and agent labels for the rest of its life with nothing on screen. Accepted, with the reasoning that more calls to a command that is not answering buy no freshness.
