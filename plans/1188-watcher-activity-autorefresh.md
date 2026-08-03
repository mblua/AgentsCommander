# Implementation Plan: #1188 The watcher activity window silently stops auto-refreshing

Status: READY_FOR_IMPLEMENTATION

Full path. Written by the architect at Step 4, enriched by `dev-webpage-ui` (Step 5, the change is TypeScript and SolidJS only, with no Rust surface) and `dev-rust-grinch` (Step 6), and certified by the architect at Step 7 after resolving every finding both raised. **Section 14 is the record of that resolution and is the authoritative reading wherever it contradicts an earlier section**, though where it changes an earlier section that section was rewritten too, so the two agree.

Every implementation decision is closed: there is no `TBD`, no competing alternative and nothing left to the implementer, who is expected to start cold with no knowledge of the discussion that produced this. Sections 12 and 13 are the enrichers' own records. Nothing in them was deleted or altered; the only Step 7 additions there are quoted `RESOLVED` and `[Superseded]` markers pointing at Section 14. **Read them for evidence, not for instructions**, since three of their recommendations were resolved differently from what they proposed.

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1188 (`Watcher activity window silently stops auto-refreshing until reopened`).
- Branch: `fix/1188-watcher-activity-autorefresh`, to be recreated from current `main` before Step 8. The branch that exists today was cut from `d7285ceb` and is stale; it is not the baseline for anything below.
- **Baseline for every coordinate and every command in this plan: `f08b82419b7943d694965af000630bf053e2922a`** (`main` == `origin/main`). Re-baselined from `d7285ceb` during Step 5 enrichment, 27 commits later. Every `file:line` below was re-verified against `f08b8241`, one by one; what moved is listed in Section 12.1.
- Delivery classification: FULL. Confirmed, not reclassified. Three properties make it non-mechanical: more than one implementation is defensible (enveloping timeout, `AbortController`, external watchdog, and three possible re-arm sites); the failure mode is silent, so a wrong design leaves the bug looking fixed; and the change lands on a poll chain whose ordering already carries four separate invariants (Section 2.3).

**Objective.** The activity poll chain must be unable to die. Whatever happens to a fetch, including never settling at all, the next round must still be scheduled, and the window must say on screen that a round failed instead of showing a frozen table with no indication.

**Non-objective.** This is not an investigation into why the promise pends. It is not a refactor of the poll, of `refresh()`, or of the window's state model.

## 2. Verified current state

### 2.1 The defect, at exact coordinates

`schedulePoll()` (`src/watchers/App.tsx:379-396`) is the only thing that arms `pollTimer`, and the only call to it after mount lives inside the `.then()` of the fetch it just issued:

```ts
// src/watchers/App.tsx:383-395
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void refresh().then(() => {
        if (disposed) return;
        settingsStore.refresh();
        schedulePoll();          // the only re-arm
      });
    }, delay);
```

`refresh()` (`:290-309`) awaits `Promise.all(ids.map((id) => PtyAPI.getWatcherActivity(id, limit)))` (`:295-297`). If that promise never settles, the `.then()` never runs, `schedulePoll()` is never called again, and `pollTimer` stays `null` for the life of the window. The window is then permanently blind to everything the snapshot alone carries: `truncated`, `possiblyMissedFrames`, `warmedUp`, `activeWatchers` and `degraded` (`:47-51`), plus every match that arrives while no `watcher_matches` push happens to land.

Two properties make it silent rather than obvious:

1. Push events still paint (`:410-418`), so the table is not blank. It is stale, and stale looks identical to "nothing has happened".
2. Nothing on screen reports the dead chain. `loadError` (`:154`, banner at `:679-683`) is only ever set from `refresh()`'s `catch`, and a promise that never settles never rejects.

The issue measured the consequence in the repo's own harness: after the hang, 10 further periods (200 s) elapsed with zero fetches, no banner and no visible indication, and releasing the hung promise afterwards did not recover it, because no timer remained to re-arm.

### 2.2 Why `finally` alone does not fix it

`p.finally(cb)` is `p.then(cb, cb)`. A promise that never settles calls neither handler. Moving the re-arm from `.then()` to `.finally()` fixes the rejection case, which is already covered by `refresh()`'s own `try/catch` (`:305-308`), and fixes nothing at all for the reported failure. **A bound on the fetch is therefore mandatory, not optional**, and it is what makes every downstream re-arm reachable.

### 2.3 The four invariants of the current chain, which the fix must not break

Established by the code and its comments at `:278-288`, `:334-352`, `:355-377` and `:385-386`:

1. **One round at a time.** The poll is chained, not fired on an interval, so a round that outlives its own period cannot stack against the per-session mutex; in "All sessions" one round is already N calls (`:385-386`).
2. **Only the newest fetch may paint.** `requestCounter` (`:288`, `:291`, `:298`, `:306`) is monotonic over every fetch, including two fetches of the same scope racing each other.
3. **The scope effect owns invalidation.** Fetches are issued by the effect at `:355-377` and by the poll; nothing else. A scope change drops the invalidated content synchronously (`:373-374`).
4. **Nothing is fetched before `scopeSettled()`** (`:156`, `:358`, `:450`).

### 2.4 What is already closed and must not be reopened

Recorded here so that enrichment does not re-litigate what the issue measured:

- The backend emits continuously: `WatcherSink::emit` calls `history.record` (`src-tauri/src/lib.rs:871`) before `delivery.deliver` (`:873`).
- Timer throttling is refuted as the cause: measured ceiling of 60 s, and a slow timer still paints. It degrades, it does not freeze.
- The stale session-list race is refuted for these episodes: it is confined to "All sessions", and `scopeIds()` never reads `agentSessions()` under a single-session scope (`src/watchers/App.tsx:173-177`).
- `get_watcher_activity` does not hang: stress-replicated at about 500k snapshots with a worst individual case of 13 ms against a 10-15 s poll period.
- The main thread was healthy: the rest of the AC UI, same process, stayed responsive during the episodes.
- The cause of the pending promise on the webview side is **not identified and is not in scope**. The defect fixed here stands on its own: the chain can die and only reopening recovers it.

### 2.5 Written precedent in this repository

The repository already solves "a Tauri `invoke` that never settles" once, and the fix below copies it rather than inventing a second shape:

- `src/sidebar/components/ProjectPanel.tsx:192` defines `export const RESTART_TIMEOUT_MS = 30_000`; `:432-450` and `:500-517` race the IPC call against a `setTimeout` rejection and clear the timer in `finally` (`:448-449`, `:515-516`).
- `src/sidebar/components/ProjectPanel.restart-toast.test.tsx:211-251` is the regression test for it: `fake.onInvoke("restart_session", () => new Promise(() => {}))`, fake timers, `advanceTimersByTimeAsync`, assert the user-visible surface appears.

`src/watchers/App.tsx` also already exports helpers purely so they can be tested in isolation: `logicalGeometry` (`:73`, rationale at `:70-72`) and `registerAll` (`:101`, rationale at `:97-99`). The new helper follows that same convention.

### 2.6 The coverage gap

The 72 tests under `src/watchers/` all pass and none exercises the poll. Re-verified at `f08b8241`: `npx vitest run src/watchers` reports `PASS (72) FAIL (0)`. The count was 71 at `d7285ceb`; #1193 added one test to `App.test.tsx` since. `src/watchers/App.test.tsx` (966 lines) contains no `vi.useFakeTimers()`, no timer advance and no reference to `schedulePoll` (`grep -c` over the three returns `0`); the two tests inside `describe("reloading the session list (#1171)")` (`:736-802`, tests at `:744` and `:774`) cover the `list_sessions` race by asserting `<select>` options, never whether events are lost. `scripts/check-test-debt.mjs` flags only skipped and placeholder tests, so the tests added here are subject to no additional constraint.

## 3. Scope

### In scope

1. Bound one activity round in time, so `refresh()` always settles.
2. Re-arm the poll from a path that runs on every outcome of a round.
3. Report a failed or abandoned round on screen, and clear that report when a round succeeds.
4. Regression coverage for a fetch that never settles, plus unit coverage of the new helper.

### Out of scope

- The root cause of the pending promise on the webview side.
- The "All sessions" stale-session-list race (separate issue).
- Timer-throttling freshness, that is, up to 60 s of stale data on refocus (separate issue).
- Cancelling the underlying IPC call. Tauri's `invoke` exposes no cancellation and `AbortController` cannot reach it; see Section 4.2.
- **The mount path.** `schedulePoll()` is reached at `:451` only after eight sequential awaits (`settingsStore.load()` at `:402`, five `register(...)` at `:410`, `:419`, `:427`, `:428` and `:429`, `reloadSessions()` at `:431`, `WindowAPI.getWatchersScope()` at `:441`). If any of those never settles, the chain is never armed at all and the window sits on "Waiting for the first sample..." forever, because `resolveView` returns `warming` for an empty snapshot list (`src/watchers/activity.ts:290-292`). This is the same defect class and it is **not fixed here**: closing it means bounding eight more await sites and changing the mount's error semantics, which is a different change with a different blast radius. **It already has its own issue, https://github.com/mblua/AgentsCommander/issues/1196 (`Watcher activity window mount path can never arm the poll chain (same defect as #1188)`), verified OPEN**; see Section 10.1. The distinction that keeps it out: the reported episodes are a window that had been working and froze, which is only reachable through the running chain.

## 4. The decided solution

### 4.1 What is implemented

Three edits in `src/watchers/App.tsx`, and nothing else in production code:

1. A `withDeadline(work, ms, message)` helper that races `work` against a timer and always clears that timer.
2. `refresh()` wraps **each per-session call** inside its `Promise.all` in `withDeadline(..., POLL_TIMEOUT_MS, POLL_TIMEOUT_MESSAGE)`, so **every** caller of `refresh()` is bounded: the poll, the scope effect (`:376`) and any future one.
3. The poll round moves into its own function whose `finally` re-arms the chain, replacing the re-arm inside `.then()`.

**Which of these two halves actually fixes the reported defect, stated exactly.** Edit 2 is the fix. Edit 3 is not a second necessary half, and an earlier draft of this plan claimed it was; that claim was false and both enrichers caught it independently (Sections 12.2.8 and 13.2 finding 2). The mechanism, verified in the file rather than reasoned about: `refresh()` (`:290-309`) owns a `try/catch` that swallows every fetch failure and returns normally, so once the deadline converts "never settles" into "rejects", `refresh()` **fulfils**, and the existing `.then()` at `:387` re-arms on its own. Edit 2 alone therefore restores the poll on `f08b8241`.

**Edit 3 is kept anyway, and the reason is not redundancy.** With only the deadline, the chain's survival rests on a property that lives 90 lines away in another function and that nothing enforces: *`refresh()` must never reject.* Any later change that puts an `await` outside its `try`, rethrows from its `catch`, or lets `refresh()` propagate, silently restores exactly this defect, in exactly its silent form. Edit 3 makes chain liveness a local, visible property of the round itself: the re-arm is in a `finally`, so it does not depend on what `refresh()` does with its errors. That is a defence against the defect's *class*, bought for about fifteen lines, in a window whose failure mode is invisible. It is hardening, and it is deliberate; it is not what makes the current bug go away.

Both edits are still mandatory for this change. What follows from getting their roles right is the demonstration in Section 8 step 5 and criterion 9.3.4, which need two separate mutations rather than one revert, because each half has to be falsified against the assertion it actually governs.

### 4.2 Decisions taken, and why the rejected shapes are rejected

These are settled, and they survived both enrichment steps. The reasoning is recorded rather than just the outcome, so that a later change argues with the reason instead of rediscovering the choice.

| Decision | Taken | Rejected, and why |
| --- | --- | --- |
| How the eternal promise is bounded | `Promise.race` against a `setTimeout` rejection | `AbortController` cannot abort a Tauri `invoke`: `PtyAPI.getWatcherActivity` (`src/shared/ipc.ts:250-254`) goes through `transport.invoke`, whose signature is `invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>` (`src/shared/transport.ts:4`) and takes no signal, and no layer down to Tauri accepts one. It would abort nothing and hide that it aborted nothing. |
| Where the deadline sits | Inside `refresh()`, around **each per-session call** within the `Promise.all` | Around the whole `Promise.all` instead: identical on every observable axis, and rejected only on a resource one. A single deadline leaves the lost call pending as an element of the `Promise.all`, and a `Promise.all` with one element still pending holds its internal values list alive, so it retains **every already-resolved sibling snapshot** for as long as the reply never comes: up to (N-1) x `ALL_SESSIONS_LIMIT` matches per abandoned round, forever. Deadlining each element makes every element settle within `POLL_TIMEOUT_MS`, so the values list is released and nothing but the callback pair survives. See Section 4.3. Around `refresh()` at the call site was also rejected: it leaves the scope effect's fetch (`:376`) unbounded and needs repeating at every future call site. |
| How the chain is kept alive | `finally` in the round wrapper, on top of the deadline | An external watchdog timer that re-arms when no round has completed needs a generation guard, because the original `.then()` can still land later and arm a second chain, which then doubles the poll rate and compounds. More state and a new failure mode, for the same guarantee. |
| Re-arm order inside `finally` | `schedulePoll()` first, then `settingsStore.refresh()` | The re-arm is the invariant that must never be skipped. Ordering it first means no statement can ever come between a round and the next one. `settingsStore.refresh()` is fire-and-forget and swallows its own failure (`src/shared/stores/settings.ts:28-32`), so nothing is lost by demoting it. |
| Backoff after a failed round | None. The period is unchanged | Backoff delays recovery, which is the property this issue exists to restore. There is no herd to protect: the peer is the same process. Enrichment reopened this on resource grounds, since a slower retry rate would also slow the accumulation of abandoned calls; the answer is that the per-element deadline above already removes the part of that accumulation that had any size to it, leaving a residue measured in bytes per lost call (Section 4.3), against which trading away recovery latency is a bad exchange. |
| Retry cap or circuit breaker after K failed rounds | None. The chain retries at the normal period for as long as the window is open | Considered at Step 7 against the accumulation in Section 4.3, and rejected on the merits, not on cost. A breaker that stops polling after K failures reproduces the exact user-visible symptom this issue reports, a window that has silently stopped refreshing, and to be honest it would need a manual retry affordance, which is new surface. It also cannot know that the next call would fail. The bound it buys is not worth reintroducing the defect in a supervised costume. |
| Bounding the poll's `settingsStore.refresh()`, which is the `+1` in Section 4.3 | No. Its lost `get_settings` is accepted as residue and documented | Three reasons, and the first alone is decisive. **It cannot be bounded from the call site at all:** `settingsStore.refresh()` returns `void`, not a promise (`src/shared/stores/settings.ts:28-32`, it is `load().catch(log)` with no `return`), so there is nothing for `withDeadline` to race. Reaching it means changing a store that every window shares, which Section 5.3 puts out of scope and Section 10.4 covers properly. **Second, a deadline there would release zero bytes even if it were possible.** What the deadline buys in `refresh()` is the release of payload held by a waiting aggregate; a lone `get_settings` whose result is dropped has no aggregate and no siblings, so its retention is the native callback pair either way. **Third, making the call conditional on a successful round was considered and rejected:** it would only help in the transport-wide case, where every `invoke` in the application is already leaking and this window's one extra is a rounding error, and it would cost real behaviour in the command-specific case, where `get_settings` still works and agent labels should keep updating while activity is broken. |
| Cancelling the abandoned call at the transport | Not attempted here | It is the only true fix for the residue and it does not belong in this window. `window.__TAURI_INTERNALS__.unregisterCallback` is exposed and would release the entries, but `invoke` never returns the two ids it registered, so reaching them means reimplementing `invoke` over `__TAURI_INTERNALS__.ipc` inside `src/shared/transport-tauri.ts`, on top of internals whose own source labels the callback map as being exposed "just for the debugging purposes". That is a transport-wide change affecting every command in the application, and it is reported as a follow-up in Section 10.4 rather than smuggled into a one-window fix. |
| Debounce before showing the banner | None. A single failed round reports immediately | The issue's requirement is that a failed cycle is visible. An 8 s round is already 615x the measured worst case, so it is never normal, and hiding the first occurrence is exactly the silence that cost a morning. |

### 4.3 The accepted consequence: abandoned fetches are not cancelled

A round that hits its deadline leaves the underlying `invoke` pending; nothing can cancel it. This section states what that costs, with the mechanism named and the arithmetic done, because an earlier draft called it "bounded" and "orders of magnitude below anything that matters", and **both of those claims were wrong**. Section 13.2 finding 1 is upheld against the draft; what follows is the resolution.

**What is actually retained, from the source.** `tauri-2.10.3/scripts/core.js:22-100` keeps a module-level `const callbacks = new Map()`. Every `invoke` calls `registerCallback` twice, once for the reply and once for the error, and each of the two handlers removes the other only when a reply arrives. A reply that never arrives therefore leaves **two Map entries per lost call**, plus the closures they hold, which reach the invoke promise's `resolve`/`reject`. `withDeadline` is given the promise, never the two identifiers, so it cannot call the `unregisterCallback` that Tauri does expose. This is not a hypothesis, it is that file.

**The rate, corrected.** The draft said "about 6 per minute", which was the poll cadence with no timeout in it. Because scheduling is chained, a broken round costs `POLL_TIMEOUT_MS + period`: 18 s focused and 23 s unfocused. That is **3.33 abandoned rounds per minute focused, 2.61 unfocused**, so about **1,600 focused or 1,250 unfocused in eight hours**, before timer throttling, which only slows it.

**How many calls that is per round depends on which failure it is, and it is not N.** Every round that ends, successfully or not, also calls `settingsStore.refresh()` from the `finally`, which issues a `get_settings` of its own (Section 5.1 D, and `settings.ts:28-32`).

| Failure | Lost calls per round | Eight focused hours, single-session |
| --- | --- | --- |
| Only `get_watcher_activity` is affected | **N** | about 1,600 |
| The transport loses replies generally | **N + 1** | about **3,200** |

The single-session transport-wide figure is twice what an earlier version of this section stated, which counted the activity calls and forgot the settings one. In "All sessions", N is the number of sessions whose replies are lost, not the size of the scope.

**The size, which is where the draft was most wrong, and what was changed because of it.** The dangerous term was never the callback entries. It was that a single deadline around the whole `Promise.all` leaves the lost call pending *as an element of that `Promise.all`*, and an aggregate with a pending element keeps its internal values list reachable, which keeps every **already-resolved sibling snapshot** alive with it. At `ALL_SESSIONS_LIMIT` (100, `activity.ts:20`) each retained snapshot carries up to 100 matches, each with a `row` of up to 256 bytes plus its captures, so a single abandoned round in a six-session scope with one bad session retains on the order of two hundred kilobytes of match payload that nothing will ever read. At 1,600 rounds that is hundreds of megabytes over a working day. There is no defensible reading of that as acceptable, which is why Section 4.2 now puts the deadline on **each element** instead: every element then settles within `POLL_TIMEOUT_MS`, the aggregate settles, and the values list is released with the siblings in it.

That conclusion survives the one input here that is documentation rather than measurement. The 256-byte row cap is a contract stated in `types.ts` and enforced on the Rust side, not something verified from the webview, so treat it as an upper figure. It does not matter: at a quarter of it, 64 bytes a row, the same eight hours still retain tens of megabytes, so the argument survives being wrong by a factor of four. Nothing about the placement decision rests on the exact byte size of a row.

**The residue that remains, stated as a shape and an estimate, which are two different kinds of claim.**

- **The shape is the load-bearing claim, and it is verified.** After the placement change, one lost call leaves two `callbacks` entries, the closures reachable from them, and the one `Error` its deadline constructed. It retains **no match payload**: no snapshot, no row, no captures. This was established from `core.js` and from the reachability chain in Section 4.2, and both enrichers confirmed it independently, including that no second large JS payload path exists. **This is the claim to check if the design is ever revisited**, and it is checkable: a heap snapshot must show no `WatcherActivitySnapshot` retained by anything reachable from `callbacks`.
- **The byte figure is an estimate and is not verified.** The residue is *on the order of a kilobyte per lost call*, which puts eight focused hours of a transport-wide outage in single-session scope, about 3,200 lost calls, in the single-digit megabytes. Treat that as an expectation of magnitude, not as a threshold anything is measured against. **It cannot be verified by reading source.** Object sizes, the real cost of an `Error` and its stack, and any state the native side queues for a reply that never came are all out of reach while the root cause of the lost reply is unknown; only a native heap and callback soak could settle it, and this change does not require one.

The distinction matters for whoever revisits this. Being wrong about the byte figure by a factor of several changes nothing about the decision below. Being wrong about the **shape**, that is, finding snapshot payload still retained, means the placement did not do what Section 4.2 claims and the design is wrong.

**Why that residue is accepted rather than eliminated.** Eliminating it needs cancellation at the transport, which is Section 10.4's follow-up and a change to every command in the application. Against it, the alternative on offer is the status quo: a window that stops polling on the first lost reply and never says so. Trading a small per-retry residue for a poll chain that recovers the instant the transport does is the right trade, and it is a trade this plan makes with its eyes open, not an accounting error. Note also what the comparison is not: without this fix the same broken transport leaks **once** and then stops, because the chain is dead. The growth is the price of retrying at all, and every design that retries pays it until `invoke` becomes cancellable.

**The `+1` is accepted for its own reasons, not by inheritance.** The `get_settings` that every round fires from the `finally` is a separate decision and it is recorded as its own row in Section 4.2: it cannot be bounded from here in the first place, since `settingsStore.refresh()` returns `void` rather than a promise, and bounding it would release nothing even if it could, because a deadline releases payload held by a waiting aggregate and there is no aggregate here. Section 10.2 already recorded that call as unable to take the chain down, which is still true; what is new is that it also contributes to this accumulation, and that is now stated in both places instead of neither.

The remaining consequences are unchanged and were confirmed by both enrichers:

- **No late paint.** If an abandoned fetch resolves later, `refresh()` is no longer awaiting it: the `await` returned at the deadline through the rejection path, so its value is discarded and never reaches `setSnapshots`/`setRows`. Invariant 2 of Section 2.3 is untouched.
- **No unhandled rejection.** `Promise.race` keeps a rejection handler attached to `work`, so a late rejection of an abandoned fetch is absorbed by the already-settled race instead of escaping to `unhandledrejection`. The per-element placement adds one more case and it is covered by the same rule: when one element ends the round, its siblings' deadlines are still armed and will each reject a few seconds later, but `Promise.all` attached handlers to every element when it subscribed, so those rejections are delivered to an already-rejected aggregate and go nowhere. Nothing is left unhandled, and each sibling's own timer is cleared by `withDeadline`'s `finally` on the way through.
- **No mutex stacking.** Invariant 1 of Section 2.3 held because a slow round delayed the next one. It still holds for a healthy round. For an abandoned one, the concern is void: the issue measured `get_watcher_activity` at 13 ms worst case, so an abandoned round is not a round still holding a per-session lock; it is a webview-side promise waiting for a reply that is not coming.

### 4.4 The constant

`POLL_TIMEOUT_MS = 8_000`.

Lower bound: it must not fire on a healthy round. The command measures 13 ms in the worst stressed case, so 8 s is about 615x that, and even a serialized "All sessions" fan-out would need more than six hundred sessions before it approached the deadline. That is a margin, not a proof: 13 ms is one measured command, not a mathematical bound on an unbounded N. The claim here is that no plausible fan-out reaches 8 s, which is weaker than "cannot fire" and is what the value actually rests on.

Upper bound: it must stay under `POLL_FOCUSED_MS` (10 s, `:52`) so that the on-screen notice arrives within one period rather than after several. **It is a user-visible service level, and that is its only justification.** An earlier draft gave a second reason, that a lower deadline stops an abandoned round from overlapping the round after it, and that reason is false: this poll is chained, not fired on an interval, so the next round is armed only inside `runPollRound`'s `finally` and **no two rounds can overlap at any value of the constant**, which is invariant 1 of Section 2.3 doing exactly its job. What does overlap later work is the abandoned `invoke` underneath, and it does so at every value of the constant, so no choice here prevents it. Both enrichers reached this independently (Sections 12.2.6 and 13.2 finding 4) and both are right. `RESTART_TIMEOUT_MS`'s 30 s is not copied: it bounds a process spawn, not a ring-buffer read.

Two orderings are asserted by **T7** (Section 9), and they are different kinds of statement. `POLL_TIMEOUT_MS < POLL_FOCUSED_MS` is the service level above and is strict. `POLL_FOCUSED_MS <= POLL_UNFOCUSED_MS` says only that an unfocused window must not poll *more* often than a focused one; equal cadences would be legitimate, so it is not strict. The prose and the test say the same thing on purpose: an earlier draft wrote both as `<` while T7 asserted the second with `toBeLessThanOrEqual`, and pointed at T5 instead of T7.

## 5. Affected surfaces: exact files and symbols

### 5.1 `src/watchers/App.tsx` (the only production file)

**A. New exported constants, immediately after `POLL_UNFOCUSED_MS` (`:53`).**

```ts
/** How long one activity round may take before it is abandoned.
 *
 *  #1188: a `finally` on the round is not enough on its own, because a promise that never
 *  settles never reaches `finally` either. This deadline is what turns "never settles" into
 *  "settles as a failure", and every re-arm below depends on it.
 *
 *  8s against a command measured at 13ms in the worst stressed case, so no plausible fan-out
 *  makes it fire on a merely slow round. It sits under POLL_FOCUSED_MS purely as a service
 *  level: a hung round is reported within one period instead of after several. It is NOT
 *  what stops rounds overlapping. Nothing can overlap here at any value, because the chain
 *  arms the next round only once this one has ended. Exported for the invariant test. */
export const POLL_TIMEOUT_MS = 8_000;

/** What the window says when a round is abandoned. Exported so the regression test asserts
 *  the exact string the window paints, rather than a substring that could drift. */
export const POLL_TIMEOUT_MESSAGE =
  "Activity refresh timed out. The list may be out of date; retrying.";
```

`POLL_FOCUSED_MS` (`:52`) and `POLL_UNFOCUSED_MS` (`:53`) also become `export const`, with no change to their values or their existing comment. They are needed by T5 and by the regression tests, which must advance by the exact period rather than by a literal copied into the test.

**B. New exported helper, placed after `registerAll` (`:122`) and before `WatchersApp` (`:124`).**

```ts
/**
 * Resolve with `work`, or reject with `message` once `ms` have elapsed, whichever is first.
 *
 * The rejection is the whole point. `work` is a Tauri `invoke`, nothing can cancel it, and a
 * lost reply leaves it pending forever; #1188 is that promise taking the only re-arm of the
 * poll down with it. Racing does not stop the call, it stops the WAIT, which is what every
 * `finally` downstream needs in order to run at all.
 *
 * `Promise.race` keeps a handler attached to `work`, so a late rejection of an abandoned call
 * is absorbed here instead of escaping as an unhandled one. The timer is cleared on both
 * paths, so a healthy round leaves nothing armed.
 *
 * Exported for its own test: the failure it exists for needs a promise that never settles,
 * which no real IPC call produces on demand.
 */
export function withDeadline<T>(
  work: Promise<T>,
  ms: number,
  message: string
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([work, deadline]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}
```

Style notes that are requirements, not preferences: bare `setTimeout`/`clearTimeout` with `ReturnType<typeof setTimeout>`, matching `pollTimer` (`:134`) and `saveTimer` (`:482`) in this file, not `window.setTimeout` (which is `ProjectPanel`'s style, in a different file). The rejection value is an `Error`, so `refresh()`'s existing `err instanceof Error ? err.message : String(err)` (`:307`) yields exactly `POLL_TIMEOUT_MESSAGE`.

**C. `refresh()` (`:290-309`): each per-session call gets the deadline.** Only the `await` expression changes.

```ts
      const fetched = await Promise.all(
        ids.map((id) =>
          withDeadline(
            PtyAPI.getWatcherActivity(id, limit),
            POLL_TIMEOUT_MS,
            POLL_TIMEOUT_MESSAGE
          )
        )
      );
```

The deadline goes **inside** the `map`, not around the `Promise.all`. Write it this way even though wrapping the aggregate looks tidier and reads the same: the two differ only in what a lost reply keeps alive, and Section 4.3 is that difference. Wrapping the aggregate leaves the lost call pending as an element of it, which keeps its values list, and every sibling snapshot in that list, reachable for the life of the window. Deadlining each element makes every element settle, so the aggregate settles and releases them.

Behaviour is otherwise identical on every axis this plan cares about. `Promise.all` still rejects the round the moment any one call fails, so the round's outcome, its message and its timing are what they were. All N deadlines are armed in the same synchronous `map`, so they expire together rather than in sequence. An empty scope arms **no timer at all**, since the `map` produces no calls (edge case 8; note that the rejected aggregate form did arm one, so this is a difference between the two shapes, not something carried over). The only visible difference is the timer count during a round, N instead of 1, all armed and cleared, which is not a cost at this scale.

Everything else in `refresh()` is unchanged: the `requestCounter` capture, both guards (`:298`, `:306`), `setSnapshots`, `setLoadError("")`, the row merge and the `catch`.

**D. New `runPollRound`, and `schedulePoll` (`:379-396`) rewritten to call it.** `runPollRound` is declared **before** `schedulePoll` in the file, for readability only: the reader meets the round before the thing that schedules it. Both orders are equally correct. An earlier draft justified the order as an initialisation constraint, which was wrong and is recorded here so nobody preserves a constraint that does not exist: both are `const` arrows in a component body that runs to completion synchronously, neither dereferences the other at definition time, and the first call to either is `schedulePoll()` at `:451`, inside `onMount`'s async continuation, long after both bindings are initialised. There is no temporal dead zone in either direction.

```ts
  /**
   * One poll round, re-armed on every path.
   *
   * #1188: the re-arm used to live inside `refresh().then()`, so any outcome that was not a
   * fulfilment ended the chain for the life of the window, and the reported outcome was no
   * outcome at all. It is in `finally` now, and the fetch has a deadline (`withDeadline`),
   * because `finally` does not run for a promise that never settles either. The re-arm is
   * the FIRST statement in the `finally`: nothing may ever come between a round and the next
   * one, and `settingsStore.refresh()` is best-effort by construction.
   */
  const runPollRound = async () => {
    try {
      await refresh();
    } catch (err) {
      // `refresh` swallows its own failures, so this is unreachable today. It is the belt for
      // anything later added outside its try: an escape here would end the chain again.
      console.error("[watchers] poll round failed:", err);
    } finally {
      if (!disposed) {
        schedulePoll();
        // The poll also refreshes the settings store, so a watcher saved from the modal turns
        // the "no watcher reaches this agent" state into "configured and waiting" without the
        // user reopening the window. There is no cross-window settings event to use instead.
        settingsStore.refresh();
      }
    }
  };

  const schedulePoll = () => {
    if (disposed) return;
    if (pollTimer) clearTimeout(pollTimer);
    const delay = document.hasFocus() ? POLL_FOCUSED_MS : POLL_UNFOCUSED_MS;
    pollTimer = setTimeout(() => {
      pollTimer = null;
      // Chained rather than fired: a round that outlives its own period would otherwise stack
      // against a per-session mutex, and in "All sessions" one round is already N calls.
      void runPollRound();
    }, delay);
  };
```

`runPollRound` never rejects (`try`/`catch`/`finally` with no rethrow), so `void runPollRound()` cannot produce an unhandled rejection. This is why the `catch` is required even though `refresh()` swallows: `void p.finally(...)` on a rejected `p` would.

**E. Nothing else changes.** No JSX, no CSS, no new testid, no new signal, no IPC, no types. The failure notice reuses `loadError` (`:154`) and the existing banner (`:679-683`, `data-ac-testid="watchers.error"`), which is the surface already built for exactly this and is already cleared by the next successful round (`:300`).

### 5.2 `src/watchers/App.test.tsx`

New tests only, listed in Section 9. The `vitest` import (`:2`) gains `vi`; the `./App` import (`:3`, today `WatchersApp, { logicalGeometry, registerAll }`) gains `POLL_FOCUSED_MS`, `POLL_UNFOCUSED_MS`, `POLL_TIMEOUT_MS`, `POLL_TIMEOUT_MESSAGE` and `withDeadline`.

### 5.3 Files deliberately not touched

`src/shared/ipc.ts`, `src/shared/transport.ts`, `src/watchers/activity.ts`, `src/watchers/styles/watchers.css`, `src/watchers/components/WatchersTitlebar.tsx`, and everything under `src-tauri/`. If the implementation needs any of them, the design is wrong and the plan must come back for revision.

## 6. Required behaviour, edge cases and behaviour on failure

| # | Situation | Required behaviour |
| --- | --- | --- |
| 1 | Round succeeds | Unchanged from today: snapshots and rows commit, `loadError` clears, next round scheduled at `POLL_FOCUSED_MS`/`POLL_UNFOCUSED_MS` by focus. |
| 2 | Round rejects (backend error) | Unchanged from today, banner and re-arm both. The banner carries the error's message, and the chain re-arms, which it already did on `f08b8241`: `refresh()` swallows the rejection in its own `catch` and fulfils, so the old `.then()` ran. An earlier draft listed the re-arm here as new; it was not. T6 locks the behaviour in rather than restoring it. |
| 3 | **Round never settles** | At `POLL_TIMEOUT_MS` the round ends as a rejection carrying `POLL_TIMEOUT_MESSAGE`; the banner shows it; the chain is re-armed and the next round fires one period later. This is the defect being fixed. |
| 4 | The round after a timeout succeeds | Banner clears (`setLoadError("")` at `:300`), table updates. No trace of the failed round remains. |
| 5 | Timed-out round superseded by a newer one | `refresh()`'s `catch` guard (`:306`) discards it: no banner, no paint. The newer round governs both. The chain is still re-armed by the `finally`, which does not consult the counter. |
| 6 | Table content during a timed-out round | Unchanged. `setSnapshots`/`setRows` are not reached, so the last good rows stay on screen under the banner. Deliberate: the banner says the list may be out of date, so blanking it would destroy information the user still wants. |
| 7 | Timeout on the fetch that a scope change issued (`:376`) | The effect has already dropped the invalidated content synchronously, so `snapshots()` is `[]` and `resolveView` gives `warming`: the window shows "Waiting for the first sample..." **with the timeout banner above it**. Accepted: the banner is what distinguishes it from a genuine warm-up, and the next poll round recovers it. |
| 8 | Empty scope ("All sessions", no agent sessions) | `ids` is empty, so the `map` produces no calls, no deadline is armed at all and `Promise.all([])` resolves immediately. No behaviour change. |
| 9 | Window closed mid-round | `onCleanup` (`:534-544`) sets `disposed` and clears `pollTimer`. The `finally` sees `disposed` and re-arms nothing. `withDeadline`'s timer is cleared when the race settles. |
| 10 | Window unfocused | Unchanged: the period is `POLL_UNFOCUSED_MS`. The deadline does not depend on focus. |
| 11 | Timer throttling in the background | The deadline is a timer too, so under throttling both stretch together. Worst case the notice is late, never wrong. Freshness under throttling stays out of scope, as stated in Section 3. |
| 12 | Push events during a dead round | Unchanged: `onWatcherMatches` (`:410-418`) still paints. It is not a substitute for the poll (Section 2.1) and is not touched. |

## 7. Compatibility and security

- **IPC.** No command, event, payload or type changes. No Rust change. No `src/shared/types.ts` change. Frontend-only, one file.
- **Persistence.** No settings key, no TOML shape, no migration.
- **Behaviour on the healthy path.** Identical. The deadline never fires (Section 4.4) and the re-arm happens at the same point in time as before, since `finally` and `then` run in the same microtask position for a fulfilled promise.
- **Security.** No new surface. `POLL_TIMEOUT_MESSAGE` is a constant with no interpolation, so nothing from the backend reaches the DOM through it. Solid escapes text nodes; the banner already renders `loadError()` as text (`:679-683`).
- **Performance.** One extra `setTimeout` per session per round, armed and cleared. That is one timer every ten seconds in the single-session scope this window spends most of its life in, and N every ten seconds in "All sessions". Not measurable. Memory under a broken IPC is the one axis with a real cost and it has its own budget in Section 4.3.
- **Windows and ConPTY.** Not reached. This is webview-side JavaScript only.

## 8. Implementation order

Each step compiles and leaves the suite green on its own.

0. Commit this plan file first, as its own commit, exactly as #1177's plan landed (`092d85cd`). **`plans/` is in `.gitignore` (`.gitignore:11`)**, so the file needs `git add -f plans/1188-watcher-activity-autorefresh.md`; a plain `git add` silently adds nothing and the plan never reaches the branch.
1. Export `POLL_FOCUSED_MS` and `POLL_UNFOCUSED_MS`; add `POLL_TIMEOUT_MS` and `POLL_TIMEOUT_MESSAGE` (Section 5.1 A). Run `npx tsc --noEmit`.
2. Add `withDeadline` (Section 5.1 B) plus its unit tests T4 and T5 and the invariant test T7. Run `npx vitest run src/watchers`. This step is complete before anything calls the helper.
3. Wire the deadline into `refresh()` (Section 5.1 C). Run the suite: it must still be green, because the deadline never fires in any existing test.
4. Add `runPollRound` and rewrite `schedulePoll` (Section 5.1 D). Run the suite.
5. Add the regression tests T1, T2, T3 and T6 (Section 9), then **falsify T1 with two separate mutations**. A regression test that never saw the defect is worthless, and one revert cannot falsify both halves of this change, because they govern different assertions. Do both, in this order, restoring the file fully between them and confirming the suite is green again after each restore.

   **Mutation M1: revert Section 5.1 C and D together**, that is, back to `f08b8241`'s production code, keeping the new tests. **T1 must fail, and it must fail at step 3, on the absent banner** (`watchers.error` is `null`), not at step 4. With no deadline there is no rejection, so `loadError` is never written and nothing is painted; the test never reaches the third-call assertion. Record that exact failure message in the PR body. Reverting **only** step 4 is not a valid mutation and must not be used: with the deadline still in place the hung round ends as a rejection, `refresh()` swallows it in its own `catch` (`:305-308`) and therefore fulfils, so the old `.then()` re-arms, the third round fires and T1 passes over code that still contains the defect. Both enrichers established this independently by execution.

   **Mutation M2: keep everything, and delete only the `schedulePoll();` line from `runPollRound`'s `finally`.** **T1 must now fail at step 4**, on the third-call assertion: exactly 2 calls where 3 are required. Step 3 still passes, because the deadline is still there and the banner still appears. This is the negative control for the re-arm site, and it is the only mutation that isolates it, since the historical `.then()` re-arm is itself reachable once a deadline exists.

   Between them the two mutations prove what each edit is for: M1 that T1 sees the shipped defect, M2 that T1's third-call assertion is genuinely load-bearing on the new re-arm rather than passing for an unrelated reason.
6. Full gates: `npm run typecheck`, `npm test`, `npm run test:debt`.

## 9. Tests and acceptance criteria

All tests go in `src/watchers/App.test.tsx`, which already imports the helpers this file exports for testing (`:2`) and already carries `// @vitest-environment jsdom` (`:1`). Add `vi` to the `vitest` import.

### 9.1 Harness requirements, which are part of the specification

These are not style notes. Getting any of them wrong produces a test that passes without exercising the defect:

- **Fake timers must be installed BEFORE `renderWithFakeTransport`.** `vi.useFakeTimers()` replaces the global timer functions; it does not convert timers already armed. The mount arms `pollTimer` at `:451`, so installing fake timers afterwards leaves that first period on real time and the test would have to wait 10 real seconds.

  **Read this before you read the precedent in Section 2.5, because they look like they contradict each other and they do not.** `ProjectPanel.restart-toast.test.tsx:211-251` does the opposite: it renders first, drives the mount with `waitFor` on real timers, and installs fake timers only afterwards (`:233`), with its own comment explaining why (`:229-232`). Both are right, and the rule underneath both is *install fake timers before the timer under test is armed*. There, the timer under test is armed by a click that happens after the mount, so there is an interval between the two in which the clock can be swapped. Here it is armed by the mount itself, so there is no such interval, and "before `renderWithFakeTransport`" is that same rule applied to this component. Copy the rule, not either test's ordering.
- **`waitFor` must not be used while fake timers are installed.** It polls on `Date.now()` and a real `setTimeout` (`src/shared/testing/ui-harness.tsx:71-92`), both of which fake timers replace, so it would spin until its own timeout.
- **Drive the mount with microtask flushes.** Every mount await resolves through `FakeTransport`, which is `async` but never timer-based (`src/shared/testing/fake-transport.ts:48-58`), so a loop of `await Promise.resolve()` settles it. The file's existing `flush()` (8 iterations, `App.test.tsx:88-90`) is not enough for the roughly ten sequential awaits the mount performs, and it was never sized for this: all six of its existing uses run *after* a `waitFor`-driven mount, never to drive one. Use a local helper of 50 iterations and then **assert the precondition** (`expect(fake.callsFor("get_watcher_activity")).toHaveLength(1)`) so a mount that did not settle fails loudly instead of passing vacuously.

  Treat 50 as a margin, not a measurement: nobody has counted the mount's real microtask depth, and the assertion is what makes an undercount safe. Do not delete that assertion to tidy the test, and if a future change makes the flush insufficient, raise the count rather than reaching for `waitFor`, which the previous bullet rules out.

- **This flush approach depends on the watchers mount needing no real timer, which is true today and is not guaranteed.** `installBrowserDomStubs` stubs `requestAnimationFrame` with a real `window.setTimeout(..., 0)` (`ui-harness.tsx:316`), which under fake timers would run only when timers are advanced, so anything on the mount path that waited on a frame would never settle under a microtask flush. Verified at `f08b8241`: `grep -rn "requestAnimationFrame" src/watchers/` returns nothing. Stated because it is load-bearing and invisible; if this window ever picks up rAF on its mount path, these tests hang rather than fail informatively.
- **Pin the focus state:** `vi.spyOn(document, "hasFocus").mockReturnValue(true)` before render, so the period is deterministically `POLL_FOCUSED_MS`. Restore in the `finally`.
- **Restore in `finally`:** `vi.useRealTimers()` and `vi.restoreAllMocks()`, plus the existing `rendered.cleanup()`.

### 9.2 The tests

**T1 (the required regression, and the reason this issue exists): the chain survives a fetch that never settles.**

Setup: `transportWith(snapshot({ matches: [match()] }))`, then override `get_watcher_activity` with a counting handler where round 1 and rounds 3+ return the snapshot and round 2 returns `new Promise(() => {})`.

**Render `<WatchersApp initialSessionId="s1" />`. This is mandatory in T1, T2 and T3, and every call count below is wrong without it.** `transportWith` registers two agent sessions (`AGENT_SESSIONS`, `App.test.tsx:48-63`), so a render with no `initialSessionId` leaves the window in "All sessions" and every round issues **two** calls instead of one. Every existing test in the file renders with it; these must too.

Two more facts the counting depends on, stated because they are invisible in the code and a cold implementer will otherwise debug them from scratch:

- **The first counted call is not the poll.** It is issued by the scope effect (`:376`) once `setScopeSettled(true)` (`:450`) makes `fetchScopeKey()` observable. `schedulePoll()` at `:451` only arms a timer and issues nothing, so step 1's single call comes from the effect.
- **The deadline timer is armed when round 2 is issued, not when the period starts.** After step 2 the clock stands at `POLL_FOCUSED_MS` and the deadline expires at `POLL_FOCUSED_MS + POLL_TIMEOUT_MS`, which is exactly where step 3 lands. This arithmetic holds for **any** positive timeout, because while round 2 is pending the chained design has no next poll timer armed at all. It does not depend on 8 s being under 10 s, and it must not be described as if it did.

Sequence and assertions, in order:

| Step | Action | Assertion |
| --- | --- | --- |
| 1 | settle the mount | exactly 1 `get_watcher_activity` call; no `watchers.error` |
| 2 | `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS)` | exactly 2 calls (round 2 fired and is hung); still no `watchers.error` |
| 3 | `await vi.advanceTimersByTimeAsync(POLL_TIMEOUT_MS)` | `watchers.error` present, `textContent` **equals** `POLL_TIMEOUT_MESSAGE`; still exactly 2 calls |
| 4 | `await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS)` | **exactly 3 calls.** This is the assertion that mutation M2 of Section 8 step 5 must break. Note that against `f08b8241` itself the test does not get this far: it fails earlier, at step 3, on the absent banner |
| 5 | flush microtasks | `watchers.error` is `null` again |

**T2: the table keeps its rows while a round is timing out.** Same setup as T1 through step 3, and additionally assert that the row painted by round 1 (`[data-ac-testid="watchers.row.s1:1"]`) is still in the DOM while the banner is showing. Guards edge case 6 against a future "clear on error".

**T3: a round abandoned at the deadline cannot paint when it settles later.** T1's setup, with round 2's handler returning the promise of a `deferred<WatcherActivitySnapshot>()` (the file already has that helper, `App.test.tsx:76-85`).

Run T1's steps 1 to 3 only. **Then, before advancing the clock any further, resolve that deferred** with `snapshot({ matches: [match({ seq: 999 })] })` and flush microtasks. Assert `[data-ac-testid="watchers.row.s1:999"]` is **absent**. Only then advance by `POLL_FOCUSED_MS` and assert the third call was issued, so the test also shows the chain recovered.

**The ordering is the test.** Resolving the deferred after round 3 has started, which an earlier draft specified, makes the assertion pass for a reason that has nothing to do with this change: round 3 has already incremented `requestCounter`, so `refresh()`'s commit guard (`:298`) would discard the stale response even if a future implementation wrongly resumed it. Resolving it while round 2 is still the newest request removes that fallback and leaves only the property being claimed, that the `await` already threw at the deadline so its continuation can never run again. Step 6 enrichment caught this (Section 13.2 finding 3) and is right; Section 12.2.2's remark that the double mechanism "does not weaken it" is the one place that section is wrong. The test exists because the structural reasoning is invisible in the diff and a future refactor that awaits the work a second time would break it silently.

Note that the "no unhandled rejection" property of Section 4.3 is deliberately **not** given a test. jsdom does not raise `unhandledrejection` on `window`, and Node's process-level detection fires on an unspecified later tick, so any test of it would assert either nothing or something timing-dependent. It is documented in `withDeadline`'s comment and guaranteed by `Promise.race` itself, which is exactly why the helper must keep using `Promise.race` rather than a hand-rolled race that drops the handler.

**T4: `withDeadline` resolves with the work's value and leaves no timer.** `await withDeadline(Promise.resolve(7), 1000, "m")` is `7`, and `vi.getTimerCount()` is `0` afterwards. No rendering, but **fake timers are still required**: `vi.getTimerCount()` calls Vitest's `_checkFakeTimers()`, which throws `"A function to advance timers was called but the timers APIs are not mocked"` when they are not installed (Section 12.2.4). Install and restore them exactly as T5 does.

**T5: `withDeadline` rejects with the message at the deadline and leaves no timer.** Work is `new Promise(() => {})`; advancing by `ms` rejects with an `Error` whose `.message` **equals** the message passed in, and `vi.getTimerCount()` is `0` afterwards. No rendering.

Attach the expectation to the promise **before** advancing the clock, that is, hold `const assertion = expect(promise).rejects.toThrow(...)`, then `await vi.advanceTimersByTimeAsync(ms)`, then `await assertion`. Advancing first leaves the promise rejected with nothing attached for a turn, which Vitest can report as an unhandled rejection and turn into a flake in an otherwise correct test.

**T6 (a fixation test, not a regression test): a rejected round also re-arms.** `get_watcher_activity` rejects on round 2 and resolves on round 3. After one period plus a microtask flush, the banner shows the backend's message; after one more period, a third call was issued. It does not depend on the deadline at all.

**Do not expect T6 to fail against `f08b8241`; it passes there and that is correct.** A rejected round already re-armed before this change, because `refresh()` catches the rejection itself and fulfils, so the old `.then()` ran. An earlier draft claimed the `.then()` failed to cover row 2 of Section 6, which was false and is now corrected in that row too. T6's job is to pin the behaviour so that the move to `finally` demonstrably does not lose it, which is worth a test even though nothing is being restored.

**T7: the two ordering invariants.** `expect(POLL_TIMEOUT_MS).toBeLessThan(POLL_FOCUSED_MS)` and `expect(POLL_FOCUSED_MS).toBeLessThanOrEqual(POLL_UNFOCUSED_MS)`, the first strict and the second not, matching Section 4.4.

The mandated comment states the **service level**, and must not claim more: a deadline at or above the period means a hung round is reported later than one period after it hangs, which is the silence this issue exists to end. It must **not** say that the bound prevents rounds from overlapping. Nothing can overlap here at any value, because the chain arms the next round only inside `runPollRound`'s `finally`; that is invariant 1 of Section 2.3 working, not something this constant buys. An earlier draft mandated exactly that false claim, which would have frozen an untruth into the suite.

### 9.3 Acceptance criteria

Objective and individually checkable:

1. `npx tsc --noEmit` exits 0 with no diagnostics.
2. `npm test` is green, and `npx vitest run src/watchers` reports `PASS (79) FAIL (0)`: the 72 verified at `f08b8241` plus exactly the seven added here (T1 to T7). No existing test is modified, renamed or deleted.
3. `npm run test:debt` exits 0.
4. T1 is falsified by **both** mutations of Section 8 step 5, and passes with the change fully applied. M1 (revert Section 5.1 C and D together) must fail T1 at **step 3**, on the absent banner. M2 (delete only the `schedulePoll();` line from `runPollRound`'s `finally`) must fail T1 at **step 4**, on the third-call assertion. The implementer records both observed failure messages in the PR body, and the suite is green again after each mutation is undone. A run in which T1 fails at the wrong step, or passes under either mutation, means the test is not testing what it claims and must be fixed before the PR opens. Reverting Section 5.1 D alone is not one of the mutations and proves nothing.
5. `git diff --stat f08b8241..HEAD` touches exactly three files: `plans/1188-watcher-activity-autorefresh.md` (force-added, see Section 8 step 0), `src/watchers/App.tsx` and `src/watchers/App.test.tsx`. Any fourth file means the design was exceeded. If the plan file is missing from that diff, the force-add was skipped.
6. **No `.then(` survives in executable code in `src/watchers/App.tsx`,** not on `refresh()` and not on anything else. Run `grep -n "\.then(" src/watchers/App.tsx` (`git grep -n "\.then(" -- src/watchers/App.tsx` is equivalent and both were run). It must return **exactly one line, and that line must be a comment**: the mandated `#1188` line in `runPollRound`'s doc block (Section 5.1 D), prose opening with ` * `. Read the match and confirm it is that comment. A second line, or a single match that is executable, means the re-arm is not where this plan puts it.

   **Do not phrase this as "returns nothing", and do not narrow the pattern back to `refresh().then`.** Section 5.1 D mandates that comment verbatim and it quotes the historical `refresh().then()` shape deliberately, because naming what was replaced is the whole job of the comment. A zero-match criterion is therefore unsatisfiable by construction: the only route to zero is editing certified specification text to suit the instrument measuring it. An earlier draft demanded exactly that and this is the correction; see Section 14.7.

   **The raw count does not move, and that is not a weakness. Read which line it is.** At `f08b8241` the one match is `:387`, `void refresh().then(() => {`, executable code and the defect itself. After the change the one match is prose. One to one in count, executable to comment in substance, and the substance is what is being measured. The widened pattern is also strictly stronger than the one it replaces, which would have missed a re-arm relocated onto any other promise; `withDeadline`'s `.finally(` is untouched by it.
7. The deadline is per call, not per round: in `src/watchers/App.tsx`, `withDeadline(` appears inside the `ids.map(` callback and `Promise.all(` is **not** an argument to `withDeadline`. It is the whole of Section 4.3's memory argument, so a reviewer who "simplifies" it back to one wrapper reintroduces an unbounded retention that no test will catch.

   **Check this by eye in the diff, and do not turn it into a `grep`.** In the code as Section 5.1 C writes it, `withDeadline(` and `PtyAPI.getWatcherActivity(` land on different lines, so any single-line pattern matches nothing and returns zero, which reads as a pass or a failure depending only on how the criterion was phrased. A multiline match (`grep -Pzo` or equivalent) would be needed to do it honestly, and a criterion that lies when it is run is worse than one a human reads.
8. Manual check, once, on Windows in a real build: open the activity window, confirm rows still arrive; there is no manual check for the timeout, because reproducing the hang on demand is exactly what the issue could not do and what T1 exists to replace.

## 10. Adjacent findings, reported and not acted on

### 10.1 The mount path has the same defect and is not fixed here

Stated in Section 3 with its coordinates. Symptom if it happens: the window shows "Waiting for the first sample..." forever, with no banner and no poll ever armed.

**It already has an issue and no new one should be filed: https://github.com/mblua/AgentsCommander/issues/1196, `Watcher activity window mount path can never arm the poll chain (same defect as #1188)`, verified OPEN at Step 7.** Its fix is to bound the mount's awaits with the same `withDeadline` and to arm the chain from a path that a pending await cannot skip. **Not folded in here**, because it changes the mount's error semantics and eight await sites, and because it is not the reported failure: #1188 is a window that had been working and froze, #1196 is a chain that was never born, and they present differently on screen.

### 10.2 `settingsStore.refresh()` is fire-and-forget by construction

`src/shared/stores/settings.ts:28-32` calls `load()` and attaches only a `catch` that logs, and returns `void` rather than the promise. If `load()` never settles, nothing hangs, because nothing awaits it. It cannot take the chain down either before or after this change. Recorded so it is not mistaken for a second instance of the same defect.

**It is, however, a contributor to the accumulation in Section 4.3, which the first version of that section missed.** The poll fires it once per round unconditionally, so under a transport-wide outage each round loses a `get_settings` reply as well as its activity calls, making the rate `N + 1` per round rather than `N`. It is accepted rather than bounded, for the three reasons in Section 4.2's row on it; the first is that it returns `void`, so there is no promise here to put a deadline on without changing a store every window shares. Stated here as well as there so that neither section reads as if the other had not considered it.

### 10.3 The banner is shared between startup failures and round failures

`loadError` is written both by the mount's outer `catch` (`:470`) and by `refresh()` (`:307`). After this change it also carries the timeout. They cannot be distinguished on screen. Deliberate: a second banner is new surface for no gain, since the messages themselves already differ. Recorded in case product later wants them told apart.

### 10.4 A Tauri `invoke` cannot be cancelled, and the whole application pays for it

Surfaced by Step 6 enrichment and confirmed at Step 7 against `tauri-2.10.3/scripts/core.js:22-100`. Every `invoke` registers two entries in a process-wide `callbacks` Map and they are removed only when a reply arrives, so **any** lost reply anywhere in this application leaks that pair permanently. This plan's window is merely the one that now notices, because it is the one that will retry.

The material fact for whoever picks this up: `window.__TAURI_INTERNALS__.unregisterCallback` is already exposed and would release the entries. What is missing is the two identifiers, which `invoke` registers internally and never returns. A cancellable invoke therefore means reimplementing the call over `window.__TAURI_INTERNALS__.ipc` inside `src/shared/transport-tauri.ts`, keeping the identifiers, and unregistering them on timeout, on top of internals whose own source comment calls the exposed map a debugging aid. `WsTransport` needs nothing: it already times out at 30 s and deletes its pending entry, so this is specific to the native path.

**Recommendation: its own issue, against the transport and not against this window.** Deliberately not folded in: it changes the code path of every command in the application, its blast radius has nothing in common with #1188's, and its absence costs this fix about a kilobyte per lost call, which Section 4.3 budgets explicitly. No issue for it existed at the time of certification.

## 11. Open decisions

**None. This section was briefly non-empty during enrichment and is empty again, deliberately and finally.**

Every choice in Sections 4, 5, 8 and 9 is closed: the constant and both of its bounds, the placement of the deadline (per call, Section 4.2), the placement of the re-arm, the statement order inside the `finally`, the declaration order of the two functions, the absence of backoff, the absence of a retry cap or circuit breaker, the absence of a debounce, the acceptance of the residual growth in Section 4.3 with its stated budget, the two mutations that falsify T1, and the ordering inside T3.

The three items Step 5 marked as needing the architect (Sections 12.2.1, 12.2.5 and 12.2.6) and the five raised at Step 6 (Section 13.2) are all resolved in **Section 14**, which records for each one what was decided and why, including the two places where the resolution went against what an enricher recommended. Nothing in this plan is left to the implementer's judgement. If something reads as ambiguous, it is a defect in this document and the plan comes back for revision rather than being resolved at the keyboard.

## 12. Enrichment: dev-webpage-ui

Step 5, frontend implementer's lens. Everything below was verified by running something, or is marked as not verified. The `codebase-memory` gate is broken (#1205), so this used plain `git` and direct file reads throughout.

### 12.1 Job A: re-baselined from `d7285ceb` to `f08b8241`

**The mechanical basis.** Only three of the files this plan depends on changed, and the shape of each change decides whether coordinates moved:

```
git diff --unified=0 d7285ceb..main -- src/watchers/App.tsx src/watchers/App.test.tsx src/shared/ipc.ts

src/shared/ipc.ts          @@ -28,2 +27,0 @@    @@ -807,10 +804,0 @@   @@ -1071,5 +1058,0 @@
src/watchers/App.test.tsx  @@ -278,0 +279,46 @@
src/watchers/App.tsx       @@ -575 +575 @@  @@ -583 +583 @@  @@ -611 +611 @@  @@ -616 +616 @@
```

- **`src/watchers/App.tsx` — no coordinate moved at all.** #1193 replaced four lines one-for-one (575, 583, 611, 616), all inside the JSX filter bar. Nothing was inserted or deleted, so every line number in this plan still points where it did. The four lines are also nowhere near the change: the poll chain is `:288-396` and the constants are `:52-53`. **They do not touch `refresh()`, `schedulePoll()`, the poll constants, or anything else this plan edits.**
- **`src/watchers/App.test.tsx` — everything from old `:279` down moved by +46.** #1193 inserted one test after old line 278.
- **`src/shared/ipc.ts` — everything from old `:30` down moved by −2**, from the two type imports dropped with `PhoneAPI`.
- **A fourth file changed that was not in the brief: `src-tauri/src/lib.rs` (+19/−6).** Its hunks are all at `:1254` and below, so Section 2.4's `:871`/`:873` are unaffected. Re-read directly at `f08b8241` to be sure: `self.history.record(...)` is still `:871` and `self.delivery.deliver(batch)` is still `:873`.

**Every coordinate in the plan, re-verified one by one at `f08b8241`:**

| Section | Coordinate | Verdict |
| --- | --- | --- |
| 2.1 | `App.tsx:379-396` `schedulePoll`, `:383-395` the timer block, `:290-309` `refresh`, `:295-297` the `Promise.all`, `:47-51`, `:154`, `:679-683`, `:410-418` | **All correct, unmoved.** The quoted `.then()` snippet matches the file verbatim, including the `settingsStore.refresh()`-then-`schedulePoll()` order. |
| 2.3 | `:278-288`, `:334-352`, `:355-377`, `:385-386`, `:288`, `:291`, `:298`, `:306`, `:373-374`, `:156`, `:358`, `:450` | **All correct, unmoved.** |
| 2.4 | `lib.rs:871`, `:873`; `App.tsx:173-177` | **All correct.** `scopeIds` is `:173-177`. |
| 2.5 | `ProjectPanel.tsx:192`, `:432-450`, `:500-517`, `:448-449`, `:515-516`; `ProjectPanel.restart-toast.test.tsx:211-251`; `App.tsx:73`, `:70-72`, `:101`, `:97-99` | **All correct.** Neither `ProjectPanel` file changed since `d7285ceb`. |
| 2.6 | test count, file length, the describe block | **Three corrections, applied inline.** `PASS (71)` → **`PASS (72)`** (measured, not inferred); 920 → **966** lines; and the describe range `:690-810` was **wrong at its own baseline** — it swept in two tests that sit outside it. Its real extent was `:690-756`, now **`:736-802`**, tests at `:744` and `:774`. |
| 3 | the mount's awaits | **Corrected inline:** the plan said "five sequential awaits" and then enumerated eight. Eight is right, and it is what Section 10.1 already says. |
| 4.2 | `src/shared/ipc.ts:252-256` | **Corrected inline to `:250-254`.** The claim it carries **still holds**: `transport.invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>` (`src/shared/transport.ts:4`, unchanged) takes no signal. The `AbortController` rejection stands. |
| 4.4, 5.1 A | `POLL_FOCUSED_MS` `:52` = 10_000, `POLL_UNFOCUSED_MS` `:53` = 15_000, both `const`, not exported | **Correct.** T7's two assertions hold on the real values. |
| 5.1 B | "after `registerAll` (`:122`) and before `WatchersApp` (`:124`)" | **Correct.** `registerAll` closes on `:122`; `WatchersApp` opens on `:124`. |
| 5.1 D, 6 | `:534-544` `onCleanup`, `:300`, `:307`, `:376`, `:470` | **All correct.** |
| 5.2 | `App.test.tsx:2` and `:3` | **Correct**, both below the insertion point. `:3` is still `import WatchersApp, { logicalGeometry, registerAll } from "./App";`. |
| 8 step 0 | `plans/` at `.gitignore:11` | **Still true**, and the file is still untracked (`git ls-files --error-unmatch` fails on it). The force-add is still required. |
| 9.1 | `ui-harness.tsx:71-92` `waitFor`, `fake-transport.ts:48-58` `invoke`, `App.test.tsx:88-90` `flush` | **All correct, all unchanged files.** |
| 9.2 | `App.test.tsx:76-85` `deferred` | **Correct**, below the insertion point. |
| 9.3.2, 9.3.5 | `PASS (78)`, `d7285ceb..HEAD` | **Corrected inline** to `PASS (79)` (72 + 7) and `f08b8241..HEAD`. |
| 9.3.6 | `grep -n "refresh().then" src/watchers/App.tsx` | **Meaningful today:** it matches exactly one line, `:387`. The criterion goes from 1 to 0. **[Superseded at Step 7, round 3: the baseline observation is right and was independently re-verified, but "1 to 0" is not reachable. Section 5.1 D mandates a comment that quotes `refresh().then()`, so this pattern still matches one line after the change. The criterion now greps `\.then(` and requires that the single surviving match be that comment rather than executable code. See Section 14.7.]** |
| 3, 10.1 | `activity.ts:290-292` `warming` | **Correct**, file unchanged. |
| 10.2 | `src/shared/stores/settings.ts:28-32` | **Correct**, file unchanged. `refresh()` is `load().catch(log)`, fire-and-forget as described. |

### 12.2 Job B: the decisions, attacked

#### 12.2.1 Section 8 step 5's demonstration does not demonstrate anything — **needs the architect**

> **RESOLVED at Step 7 (Section 14.3). Upheld, and taken further than recommended.** Section 8 step 5 and criterion 9.3.4 now specify two mutations rather than one revert, because reverting C+D falsifies the banner but leaves the re-arm without a negative control. T6 is relabelled a fixation test and the same false claim was removed from Section 6 row 2. Nothing here is open; read Sections 8 and 9.3 for what to do.

This is the most consequential finding, because it is the step that proves the regression test is real, and acceptance criterion 9.3.4 depends on it.

Section 8 is sequential: step 3 wires the deadline into `refresh()`, step 4 replaces the `.then()` re-arm with `runPollRound`'s `finally`. Step 5 then says to revert **step 4** and watch T1 fail.

It will not fail. With step 3 still applied, the hung round no longer hangs — it ends as a rejection at `POLL_TIMEOUT_MS`. `refresh()` catches that itself (`:305-308`) and therefore **fulfils**. A fulfilled promise runs `.then()`. So the old re-arm fires, the third round is issued, and T1's step-4 assertion passes on code that still contains the defect.

Executed rather than reasoned about, running both shapes as plain JavaScript:

```
5. deadline applied + old .then() re-arm -> schedulePoll would be called again=true
```

The revert set must be steps **3 and 4** together — that is, the whole production change — which is also the only state that makes T1's step-3 assertion (the banner) meaningful, since without the deadline no banner ever appears. **Recommended correction, for the architect to make:** step 5 and criterion 9.3.4 both say "revert Section 5.1 C and D" rather than "revert step 4". I have not made it; it changes an acceptance criterion.

A related plan defect: **T6 passes on unfixed code too.** A round that *rejects* already re-armed before this change, for the same reason — `refresh()` swallows the rejection and fulfils. T6 is a lock-in test, not a regression test. Section 9.2 currently claims that the old `.then()` failed to cover this case; that claim is false and must be removed so an implementer does not expect T6 to fail first.

#### 12.2.2 `withDeadline` is correct and type-clean — confirmed, with evidence

Compiled the helper exactly as Section 5.1 B writes it, plus the Section 5.1 C call site, with **this repo's own `tsc` 5.9.3** and this repo's `compilerOptions` (`strict`, `target ES2021`, `module ESNext`, `moduleResolution bundler`, `isolatedModules`), in a file outside the repo:

```
tsc --noEmit --strict --target ES2021 --module ESNext --moduleResolution bundler
    --esModuleInterop --skipLibCheck --forceConsistentCasingInFileNames --isolatedModules
exit=0
```

I went looking for a specific failure and did not find it: `Promise.race` is typed `Promise<Awaited<T[number]>>`, and `Awaited<T>` does not reduce to `T` for an unconstrained generic, so `return Promise.race([...])` against a declared `Promise<T>` was a plausible error. It compiles, including when `withDeadline` is called through a type variable (`<X>(p: Promise<X>): Promise<X>`). Noting the version, because `Awaited` behaviour is TypeScript-version-sensitive: verified on **5.9.3**, the version in this repo's lockfile.

`ReturnType<typeof setTimeout>` needs no defence — `pollTimer` (`:134`) and `saveTimer` (`:482`) already use it in this same file and compile today. `noUnusedParameters` is not enabled, so the `_` placeholder is safe either way.

The runtime semantics the plan asserts all hold. Executed:

```
1. work wins -> value=7 elapsed=0ms          (timer cleared; otherwise the process hangs 60s)
2. deadline wins -> instanceof Error=true  banner_text_equals_message=true
3. late rejection absorbed -> unhandled=[]
4. late resolution -> refresh returned "caught"  continuationRan=false
6. empty scope -> [] resolved immediately
```

Line by line against the plan's claims:

- **The `Error` lands in the banner as exactly `POLL_TIMEOUT_MESSAGE`.** `refresh()`'s `err instanceof Error ? err.message : String(err)` (`:307`) yields the message verbatim, so T1's `toBe` (not `toContain`) is safe. **Confirmed.**
- **`Promise.race` absorbs a late rejection of the abandoned work.** Zero unhandled rejections. Section 4.3 is right, and so is its warning that a hand-rolled race would lose this. **Confirmed.**
- **No late paint.** The `await` already threw, so the continuation never resumes no matter what the abandoned promise later does. `setSnapshots` is unreachable. Section 4.3 and T3's structural argument are both right. **Confirmed.** Worth recording that T3 would also pass for a second, independent reason — `requestCounter` would have advanced past it — so T3 cannot tell the two mechanisms apart. That does not weaken it; it just means it is not a sharp test of the one the plan cares about. **[Overruled at Step 7: it does weaken it, and Section 13.2 finding 3 is right. T3 is now specified to resolve the deferred while round 2 is still the newest request, so the counter cannot supply the answer. See Section 9.2 T3 and Section 14.4.]**
- **Edge case 8 holds:** `Promise.all([])` resolves immediately and the timer is armed and cleared in the same turn. **[Superseded at Step 7: true of the aggregate form this was written against, but the deadline is now per element, so an empty scope arms no timer at all. The outcome is unchanged. Section 6 row 8 is normative and was already correct; Section 5.1 C now says so explicitly. See Sections 4.2 and 14.4.]**

#### 12.2.3 The Section 9.1 harness spec is right — but it contradicts the precedent this plan cites

Every claim checks out against the real harness:

- **`waitFor` is unusable under fake timers.** `src/shared/testing/ui-harness.tsx:71-92` loops on `Date.now() - started < timeoutMs` and awaits `setTimeout(resolve, 10)`. `vi.useFakeTimers()` replaces both, so the loop never advances and the inner await never resolves: it does not even time out, it just hangs until Vitest kills the test. **Confirmed, coordinates exact.**
- **`FakeTransport` is timer-free.** `fake-transport.ts:48-58` is a plain `async invoke` that awaits a handler. Microtask flushing genuinely settles it. **Confirmed, coordinates exact.**
- **Nothing else on the watchers mount path needs a real timer.** I checked the one thing that could have broken this: `installBrowserDomStubs` stubs `requestAnimationFrame` with a real `window.setTimeout(..., 0)` (`ui-harness.tsx:316`), which under fake timers would only run when timers are advanced. It does not matter here — `grep -rn "requestAnimationFrame" src/watchers/` returns nothing. **The mount is safe to drive with microtasks alone.** This is load-bearing and the plan does not say it; if a future mount picks up rAF, the flush approach dies silently.

**The contradiction.** Section 2.5 points the implementer at `ProjectPanel.restart-toast.test.tsx:211-251` as the written precedent. That test does the **opposite** of Section 9.1's first bullet: it renders first, drives the mount with `waitFor` on **real** timers, and installs `vi.useFakeTimers()` only afterwards (`:233`), with a comment saying exactly why (`:229-232`).

Both are correct, for a reason the plan never states: in `ProjectPanel` the timer under test is armed by a **click after** the mount, so you can switch clocks in between. In the watchers window the timer under test is armed **by the mount itself** (`schedulePoll()` at `:451`), so there is no "in between" — mount on real timers and the first period is already real. The rule is *install fake timers before the timer you need to control is armed*; "before `renderWithFakeTransport`" is that rule's consequence **here**, not a general one.

An implementer starting cold will read the precedent the plan sent them to and find it contradicting the spec, with nothing to resolve it. **Recommend the architect add that one sentence to 9.1.** I have not added it: it is spec text, not a coordinate.

**On the 8-iteration `flush()`.** I could not disprove the plan and did not try to defend it either. What I can say: no existing test uses `flush()` to drive a mount — all six uses (`:489`, `:588`, `:767`, `:796`, `:846`, `:853`) come *after* a `waitFor`-driven mount — so 8 was never sized for this. Eight sequential awaits, each through an `async` `invoke` that itself awaits, costs well over 8 microtask turns. **I did not measure the real number**, because measuring it means adding a test to the repo and this step forbids touching it. The plan's own mitigation is what makes this safe regardless: asserting `expect(fake.callsFor("get_watcher_activity")).toHaveLength(1)` right after the flush turns a wrong constant into a loud failure instead of a vacuous pass. Keep that assertion; treat 50 as a margin, not a measurement.

#### 12.2.4 T4 cannot run as written

T4 says "No rendering" and asserts `vi.getTimerCount()` is `0`. Without fake timers installed that call **throws**. From this repo's own `node_modules/vitest`:

```js
_checkFakeTimers() {
  if (!this._fakingTime) throw new Error("A function to advance timers was called but the timers APIs are not mocked. Call `vi.useFakeTimers()` in the test file first.");
  return this._fakingTime;
}
getTimerCount() { if (this._checkFakeTimers()) return this._clock.countTimers(); return 0; }
```

So T4 needs `vi.useFakeTimers()` / `vi.useRealTimers()` exactly like T5, even though it never advances anything. Corrected inline in 9.2, since it is a fact about the tooling rather than a design choice. Note that the assertion is still meaningful under fake timers: `Promise.race` settles from an already-resolved `work` through microtasks, which fake timers do not block, and `.finally` runs before the `await` returns — so the timer is provably cleared by the time the count is read.

#### 12.2.5 `runPollRound` before `schedulePoll`: right call, wrong reason — **needs the architect**

> **RESOLVED at Step 7 (Section 14.3). Applied as recommended.** Section 5.1 D keeps the order for readability and states the correction explicitly. Nothing here is open.

Section 5.1 D requires `runPollRound` to be declared before `schedulePoll` "so the reference is initialised before any timer can fire". **That justification is vacuous.** Both are `const` arrow functions in the component body, which runs to completion synchronously. Neither *evaluates* the other at definition time — `schedulePoll` only reads `runPollRound` when its timer fires, and the first call to either is `schedulePoll()` at `:451`, inside `onMount`'s async continuation, long after both bindings exist. The reverse order has no temporal dead zone either. Solid's `createEffect` at `:355` calls `refresh()`, never `schedulePoll`, so there is no earlier path.

The ordering is still the one I would write, for readability: the reader meets the round before the thing that schedules it. **Keep the ordering, drop or replace the reason** — a plan that states a false invariant teaches the next implementer something wrong, and a cold implementer may preserve a constraint that does not exist at some real cost.

#### 12.2.6 The upper bound on `POLL_TIMEOUT_MS`: one of its two reasons does not hold — **needs the architect**

> **RESOLVED at Step 7 (Section 14.3). Applied as recommended.** 8 s and T7 both stay, reframed as a service level in Section 4.4, in the production comment of Section 5.1 A and in T7 itself. The lower bound was softened too. Nothing here is open.

Section 4.4 gives two reasons for `POLL_TIMEOUT_MS < POLL_FOCUSED_MS`, and T7 requires a comment stating that "a deadline above the period lets an abandoned round overlap the round after it, which is the stacking that invariant 1 of Section 2.3 exists to prevent."

**Rounds cannot overlap, at any value of the constant, in the design this plan adopts.** The next round is armed only inside `runPollRound`'s `finally`, i.e. only once the current round has ended. Raise the deadline to 30 s and a hung round simply ends at t+30 s and arms the next for t+40 s. This is Section 2.3 invariant 1 itself: *chained, not fired on an interval*. The overlap the comment describes is a property of an interval-driven poll, which this is not.

What a longer deadline actually costs is the plan's **second** reason, which is sound: the banner arrives after several periods instead of within one, and the user stares at a frozen table for that much longer. That alone justifies the bound and justifies T7.

So: **keep `POLL_TIMEOUT_MS = 8_000`, keep T7, and rewrite the reason.** As written, T7's mandated comment would assert something untrue in the test suite forever, and the plan would contradict its own Section 2.3. I have not rewritten it; the wording is the architect's.

#### 12.2.7 T1 is under-specified in two ways that change its numbers

Both would send a cold implementer to a failing or wrong test, and neither is a design question.

1. **The scope is never stated.** T1's assertions count `get_watcher_activity` calls exactly (1, then 2, then 3). That arithmetic only holds under a **single-session** scope. `transportWith` registers two agent sessions (`AGENT_SESSIONS`, `App.test.tsx:48-63`), so rendering `<WatchersApp />` without `initialSessionId` puts the window in "All sessions" and every round issues **two** calls, making every number in T1's table wrong. The existing tests all render `<WatchersApp initialSessionId="s1" />`; T1, T2 and T3 must say so explicitly.
2. **The first fetch does not come from `schedulePoll`.** T1 step 1 expects exactly one call after the mount settles. That call is issued by the **scope effect** (`:376`), which runs once `setScopeSettled(true)` (`:450`) makes `fetchScopeKey()` observable; `schedulePoll()` at `:451` only *arms a timer* and issues nothing. The distinction matters the moment someone debugs a count that is off by one, and it is invisible in the plan today.

Also worth stating explicitly for the implementer, since T1's timing is exact: the deadline timer is armed when round 2 is **issued**, so after step 2 the clock is at `POLL_FOCUSED_MS` and the deadline fires at `POLL_FOCUSED_MS + POLL_TIMEOUT_MS` — step 3's `advanceTimersByTimeAsync(POLL_TIMEOUT_MS)` lands exactly on it. That timing is correct for any positive timeout: while round 2 is pending, the chained design has no next poll timer armed. It does **not** depend on 8 s being below 10 s.

#### 12.2.8 Confirmed, with the reason, where I agree

- **`finally` alone is insufficient, but the deadline alone is sufficient for the current code.** A promise that never settles reaches no `finally`; that half of the reasoning holds. But `refresh()` catches and swallows every fetch failure, so once the deadline converts the hang into a rejection, `refresh()` fulfils and the existing `.then()` re-arms. The `disposed` check in that `.then()` intentionally suppresses re-arm after teardown; it is not an uncovered race. `runPollRound` can still be retained as explicit hardening against a future rejecting `refresh()`, but Sections 4.1 and 12.2.8 must not call it necessary to fix the present defect.
- **The deadline belongs inside `refresh()`, not at the call site.** The scope effect's fetch (`:376`) is a real second caller and it would otherwise stay unbounded. One deadline per round rather than per session is also the only shape that keeps the timer count independent of scope size.
- **Re-arm first, `settingsStore.refresh()` second.** `settings.ts:28-32` is `load().catch(log)` — it cannot throw and nothing awaits it, so demoting it costs nothing and the invariant that must never be skipped goes first. Agreed.
- **No backoff, no debounce.** Both would extend exactly the silence this issue exists to end.
- **Reusing `loadError` and the existing banner.** No new testid, no new signal, and it already clears on the next good round at `:300`. The right call; 10.3's note about the shared banner is the honest cost.
- **`void runPollRound()` cannot produce an unhandled rejection**, because `runPollRound` has `try`/`catch`/`finally` and never rethrows. The plan's reason for keeping the otherwise-dead `catch` is sound: without it, `void refresh().finally(...)` on a rejecting `refresh()` would escape.

#### 12.2.9 What I could not verify

- **T1 through T7 were not written or run.** This step forbids touching the repo, so their feasibility is argued from the harness and from executing the promise semantics in isolation, not from a green run. The claims I could execute, I executed; 12.2.1 is the one place that changed my answer.
- **The exact microtask depth of the mount** (12.2.3). Not measured, for the same reason.
- **The 27 commits' effect beyond the files this plan names.** I checked every file the plan cites and the four that changed; I did not audit the other 23 commits for indirect effects. `npx vitest run src/watchers` is green at `f08b8241` (72/72), which is evidence but not proof.
- **Nothing about the Rust side**, beyond confirming `lib.rs:871`/`:873` still read as Section 2.4 quotes them.

## 13. Enrichment: dev-rust-grinch

Step 6, adversarial plan review. The mandatory Codebase Memory gate remains blocked by #1205, so this review used the explicitly authorized fallback: the complete issue and plan, targeted direct reads, plain `git`, the repository's existing tests/toolchain, and isolated executable promise models. No production or test file was changed.

### 13.1 Evidence established independently

- Baseline was exact and clean: `HEAD == main == origin/main == f08b82419b7943d694965af000630bf053e2922a`; zero status lines.
- `npx tsc --noEmit` exited 0. `npx vitest run src/watchers` independently returned **2 files / 72 tests passed**.
- I executed the four relevant promise shapes in Node with assertions. Result:

  ```json
  {"unfixedHung":0,"deadlineOldThenHung":1,"oldThenRejected":1,"fullFixHung":1}
  ```

  The numbers are re-arm counts. They prove both that the historical pending promise kills the old chain and that either a deadline plus the old `.then()` or the full proposed shape re-arms it. A plain backend rejection already re-arms on the historical code.
- The native leak is not hypothetical. Tauri 2.10.3's injected `scripts/core.js:22-100` holds a global `callbacks` `Map`; every `invoke` registers separate success and error callbacks and unregisters them only when one reply arrives. `withDeadline` has neither callback id and cannot remove either entry. By contrast, this repo's `WsTransport.invoke` has its own 30 s timeout and deletes `pending`, so the unbounded case is specifically the native Tauri path used by the activity window.
- The mount defect already has a concrete follow-up: open issue **#1196**, `Watcher activity window mount path can never arm the poll chain (same defect as #1188)`.
- I corrected three factual errors inline in Section 12 without deleting its findings: T6 is falsely described elsewhere as a regression, T1's timer arithmetic does not depend on 8 s < 10 s, and the deadline alone is sufficient for the current swallowed-error `.then()` chain. Findings 2 and 4 below record the consequences.

### 13.2 Grinch Review — findings the architect must resolve

> **ALL FIVE RESOLVED at Step 7 (Section 14.4).** Findings 2, 3, 4 and 5 are upheld and applied. Finding 1 is upheld against the plan's wording and its magnitude argument, but its proposed remedy is declined: the failure policy is not reopened, and the unbounded term is removed instead by moving the deadline inside the `map` (Sections 4.2, 4.3 and 5.1 C), with the residue characterised explicitly and transport cancellation recorded as a follow-up in Section 10.4. Round 2 then corrected the rate this section's finding implied, from `N` to `N + 1` per round under a transport-wide outage, and demoted the byte figure from a threshold to an unmeasured estimate; see Section 14.6. Nothing here is open.

1. **What: Section 4.3 calls abandoned native invokes “bounded and acceptable”, but they grow without a fixed bound.**
   - **Why:** “bounded by how long the window stays open” is a lifetime, not a bound; this window can remain open overnight. Because scheduling is chained, the broken-path rate is not the stated 6/minute but one round per `POLL_TIMEOUT_MS + period`: about 3.33/minute focused or 2.61/minute unfocused before timer throttling. That is still about 1,600 or 1,252 abandoned rounds in eight hours. Each lost native invoke leaves **two entries** in Tauri's callback map. In All-agents scope a round creates N invokes, not one. If one of them is lost, `Promise.all` also retains the already-resolved sibling snapshots and its values array until the missing promise settles; if replies are lost transport-wide, all N callback pairs remain. The proposed unconditional `settingsStore.refresh()` can add one more unbounded `get_settings` invoke per failed round. Ring buffers are bounded; this growth is not, so “orders of magnitude below” has no evidentiary basis.
   - **Fix:** reopen the failure policy. The architect must choose and test a genuinely bounded native strategy: transport-level cancellation/timeout that unregisters callbacks, a finite retry/circuit policy with an honest recovery action, or another mechanism that releases the webview/native callback state. Backoff alone only lowers the slope and is still unbounded. If the choice is nevertheless to accept growth, the plan needs a measured native soak, an explicit memory/callback budget and an acceptance threshold; the current closure-only estimate is insufficient.

2. **What: the plan's claim that both production halves are necessary is false, and the prescribed regression demonstration cannot fail where it says.**
   - **Why:** `refresh()` owns its `try/catch` and fulfils after a fetch rejection. Adding Section 5.1 C alone turns “pending forever” into a caught timeout; the existing `.then()` then re-arms. Reverting only D therefore leaves T1 green, exactly as 12.2.1 reports. Reverting C and D reaches the historical bug, but T1 then fails first at step 3 because there is no timeout banner—not at the step-4 third-call assertion promised by Section 8. T6 is also a fixation test: a backend rejection already fulfils `refresh()` and re-arms on `f08b8241`; Section 9.2's statement that `.then()` failed that case is wrong.
   - **Fix:** the architect must decide whether D remains as explicit future-proofing or is removed as unnecessary scope. If it remains, reframe it honestly. For proof, either (a) revert C+D and require only that T1 fails against the baseline, recording the actual missing-banner failure, or (b) run two mutations: remove C to prove timeout visibility, and separately remove the `schedulePoll()` from the new `finally` while retaining C to prove the third-round assertion. Reverting D to the historical `.then()` is not a negative control for re-arm. Label T6 as fixation and remove the false claim.

3. **What: T3's ordering lets `requestCounter` mask the late-paint defect it claims to isolate.**
   - **Why:** T3 currently runs through T1 step 4 before resolving round 2. Round 3 has therefore incremented `requestCounter`, so the old request is stale and cannot paint even if a future implementation incorrectly resumes it. The test can pass while a timed-out request still paints when it resolves during the interval before the next round. Section 12.2.2 identifies the second mechanism but incorrectly says it does not weaken the test.
   - **Fix:** after T1 step 3, resolve the timed-out deferred **before** advancing `POLL_FOCUSED_MS`; flush and assert row `s1:999` is absent while round 2 is still the newest request. Then advance the period and assert round 3 recovery. This tests the promised structural discard rather than the counter fallback.

4. **What: the stacking refutation holds for chained wrapper rounds, but the timeout invariant and T7 are internally inconsistent and overclaim what `<` guarantees.**
   - **Why:** `runPollRound` cannot overlap another scheduled `runPollRound` at any timeout value; the next timer is armed only after it ends. An abandoned underlying invoke, however, overlaps later work at **every** timeout value, and the independent scope effect can overlap a poll when a scope change happens near an already-armed timer. Therefore `POLL_TIMEOUT_MS < POLL_FOCUSED_MS` prevents neither category. It does provide a clear UX SLA: report a hung round within one focused period. Eight seconds remains a plausible value—nothing reviewed contradicts it—but the 13 ms single-command observation is not a mathematical bound for an unbounded N-session fan-out, so “cannot fire” is too strong. There are also two mechanical contradictions: Section 4.4 points to T5 although the invariant is T7, and Section 4.4 states strict `POLL_FOCUSED_MS < POLL_UNFOCUSED_MS` while T7 permits equality with `toBeLessThanOrEqual`.
   - **Fix:** retain 8 s and T7 only as an explicit UX deadline unless the architect supplies another true invariant. Rewrite the T7 comment, correct T5→T7, and make the prose/test agree on `<` versus `<=`. The inline correction in 12.2.7 also removes the false claim that T1's clock arithmetic depends on 8 s < 10 s.

5. **What: excluding the mount path is a real issue boundary, not a partial fix for #1188, but the plan omits its existing tracker.**
   - **Why:** the reported state requires a successful mount and a chain that later dies. A never-born chain has a different visible state, eight different awaits and different listener/error semantics. Folding that work in would materially enlarge the blast radius. Issue #1196 already captures exactly that distinction and is OPEN.
   - **Fix:** keep mount out of #1188 and link #1196 explicitly in Sections 3 and 10.1. No new design choice is needed here.

### 13.3 Step 5 claims that held

- The fake-timer guidance is correct for this component. The apparent `ProjectPanel` contradiction is resolved by when the timer under test is armed: after a click there, during mount here. Section 9.1 should state that rule explicitly.
- T1/T2/T3 need `<WatchersApp initialSessionId="s1" />`; otherwise two registered agent sessions double every exact call count. The first counted fetch is the scope effect, not the poll timer. Both Step 5 observations are correct.
- T4 needs fake timers before `vi.getTimerCount()`. T5 should attach its `.rejects` expectation to the promise before advancing time, so Vitest never observes a temporarily unhandled rejection.
- `runPollRound` before `schedulePoll` is readable but not required by initialization order; both bindings exist before the mount continuation can call either. Keep the order if desired and remove the false TDZ reason.
- The deadline belongs around the whole `Promise.all` inside `refresh()`; the scope effect is a second caller and per-session timers add no useful semantics. **[Superseded at Step 7: "inside `refresh()`" holds and is why the scope effect is covered, but the deadline is now per element, not around the aggregate. Per-element timers do add a semantics this bullet did not consider, namely that they let the aggregate settle and release the sibling snapshots this reviewer's own finding 1 identified. See Sections 4.2, 4.3 and 14.4.]** `Promise.race` also correctly absorbs a late work rejection and clears its own timer. Reusing `loadError`, keeping old rows under the banner, and re-arm-before-best-effort-settings are coherent choices if D remains.
- No debounce is justified. The “no backoff” choice, however, is no longer closed because Finding 1 gives backoff/circuit behavior a resource-safety purpose that Section 4.2 never considered.

### 13.4 What remains unverified

- T1-T7 do not exist, so I could not run their exact Solid/Vitest implementations without violating this step's plan-only write restriction. The existing 72 watcher tests and `tsc` are green.
- I did not reproduce the intermittent lost native reply or run a native overnight soak. The Tauri callback retention mechanism is source-proven; its exact per-entry heap cost is not measured.
- I did not reopen any of the five hypotheses closed by the issue.

## 14. Architect's resolution, Step 7

Verdict: **READY_FOR_IMPLEMENTATION**, recorded at round 1, re-certified at round 2 after both enrichers reviewed the one change round 1 made without them, and re-certified again at round 3. Every finding from Sections 12 and 13 is resolved below, each one either applied to the plan or closed with a reason. Two are closed against the enricher's own recommendation and both are marked. **Section 14.6 records what round 2 changed**, which was arithmetic and presentation, not design, and **Section 14.7 records what round 3 changed**, which was one self-contradicting acceptance criterion and nothing else. Sections 12 and 13 are preserved untouched as evidence; **where they and this section differ, this section governs**, and the body of the plan has been rewritten to agree with it, so the implementer never has to arbitrate.

### 14.1 What I verified myself before deciding

The `codebase-memory` gate is still broken (#1205), so this used the authorized fallback: plain `git`, direct file reads and the repository's own toolchain.

- Baseline: `HEAD == main == origin/main == f08b82419b7943d694965af000630bf053e2922a`, `git status --porcelain` empty.
- `npx vitest run src/watchers`: **2 files, 72 tests passed, exit 0**. Independently reproduced, so criterion 9.3.2's `PASS (79)` is 72 plus the seven added here.
- `refresh()` at `src/watchers/App.tsx:290-309` and the `.then()` re-arm at `:387` read exactly as quoted, including the `try/catch` that swallows every fetch failure. This is the fact the whole two-halves question turns on and it is in the file.
- **Tauri's callback retention, read in the crate rather than taken on trust:** `tauri-2.10.3/scripts/core.js:22-100`, in this machine's cargo registry, holds `const callbacks = new Map()`; `invoke` calls `registerCallback` twice, once for the reply and once for the error, and each removes the other only when a reply arrives. `unregisterCallback` is exposed on `__TAURI_INTERNALS__` but `invoke` never returns the two identifiers. Section 13.2 finding 1's mechanism is confirmed verbatim.
- `ALL_SESSIONS_LIMIT` is 100 and `SINGLE_SESSION_LIMIT` is 500 (`activity.ts:19-20`); `WatcherMatchPayload.row` is capped at 256 bytes (`types.ts:141-160`). These are what size the retention arithmetic in Section 4.3.
- Issue **#1196 is OPEN** with exactly the title Section 13.1 gives it. A search of open and closed issues for the transport-level cancellation follow-up found nothing, so Section 10.4 correctly says no issue exists for it.
- `AGENT_SESSIONS` (`App.test.tsx:48-63`) does register two sessions, so the `initialSessionId="s1"` point is real. `deferred` is at `:76-85`, `flush` at `:88-90` with 8 iterations, and `grep -rn "requestAnimationFrame" src/watchers/` returns nothing.

### 14.2 The disagreement, settled

**Both enrichers were right about the mechanics, and the plan was wrong.** They did not actually disagree with each other: Section 12.2.8 and Section 13.2 finding 2 say the same thing, and Section 13.1 records that Step 6 corrected Section 12 inline on this exact point, which is why 12.2.8 now reads as it does. The deadline alone re-arms the chain on `f08b8241`, because `refresh()` swallows the rejection and fulfils, so the historical `.then()` runs. The draft's "both halves are required and neither is sufficient alone" was false and is gone.

**`runPollRound` stays.** Not as a necessary half, but because without it the chain's survival depends on `refresh()` never rejecting, a property that lives in another function and that nothing enforces or tests. This defect is silent; a future `await` outside that `try` would restore it silently. Fifteen lines to make chain liveness local and visible is the right price, and Section 4.1 now says exactly that instead of overclaiming. The consequence is that D can no longer be demonstrated by reverting it, which is what Section 8 step 5 and criterion 9.3.4 now handle with two mutations.

### 14.3 Section 12 findings

| # | Finding | Resolution |
| --- | --- | --- |
| 12.2.1 | Step 5's demonstration proves nothing; T6 mislabelled | **Applied, and further than recommended.** Step 5 now specifies two mutations, M1 (revert C+D, must fail at T1 step 3 on the absent banner) and M2 (delete only the `schedulePoll()` from the `finally`, must fail at T1 step 4). Dev proposed reverting C+D; that alone leaves the re-arm without a negative control, which Step 6 caught. T6 is relabelled a fixation test in 9.2 and the false claim is removed from Section 6 row 2 as well, which neither enricher flagged. |
| 12.2.2 | `withDeadline` is correct and type-clean | **Accepted as confirmation.** No change, except that its closing remark about T3 is overruled: see 13.2 finding 3. |
| 12.2.3 | Harness spec right, but contradicts the `ProjectPanel` precedent | **Applied.** Section 9.1's first bullet now states the underlying rule, install fake timers before the timer under test is armed, and why the same rule produces opposite orderings there and here. The rAF observation is added as its own bullet because it is load-bearing and was nowhere in the plan. |
| 12.2.4 | T4 cannot run as written | **Kept as applied.** Correct fact about the tooling. |
| 12.2.5 | Declaration order justified by a false invariant | **Applied as recommended.** Order kept for readability, reason replaced, and the correction stated explicitly so nobody restores a constraint that does not exist. |
| 12.2.6 | One of the two upper-bound reasons does not hold | **Applied as recommended.** 8 s and T7 both stay; Section 4.4, the production comment in 5.1 A and T7's mandated comment are rewritten as a service level. The lower bound is also softened: 13 ms is one measurement, not a bound on N, so "cannot fire" became "no plausible fan-out reaches it". |
| 12.2.7 | T1 under-specified: scope, and the origin of the first call | **Applied as recommended**, both, plus the deadline-arming arithmetic, all stated in T1 where the implementer will be counting calls. |
| 12.2.8 | Where dev agrees | **Accepted**, with the first bullet's conclusion adopted in 14.2. |

### 14.4 Section 13 findings

**Finding 1, the accumulation. Upheld against the plan, and closed without reopening the solution.** The draft's "bounded" and "orders of magnitude below anything that matters" were both indefensible and are deleted. What I did not accept is the proposed remedy. Reopening retry, circuit-breaker or cancellation policy is the wrong move here, for reasons now recorded in Section 4.2: a breaker reproduces this issue's own symptom under supervision and needs new UI to be honest; transport cancellation is real but is a change to every command in the application and is now Section 10.4's follow-up.

What I did instead is the part of the finding that had the size in it. Grinch observed, correctly, that `Promise.all` retains already-resolved sibling snapshots while one element stays pending. That is not a fact about retry policy, it is a fact about **where the deadline sits**, and it was the only unbounded term with real magnitude behind it: at 100 matches per snapshot with rows up to 256 bytes, one bad session in a six-session scope retains on the order of two hundred kilobytes per abandoned round, and about 1,600 rounds accumulate in eight focused hours. Moving the deadline inside the `map`, so every element settles and the aggregate releases its values list, removes that term entirely and leaves a residue with no snapshot payload in it. Section 4.3 states the mechanism, the corrected rate (3.33 rounds per minute focused, 2.61 unfocused, not the draft's 6) and the per-round call arithmetic.

**This placement was made at Step 7 without either enricher having reviewed it, and was reviewed by both in round 2.** Both confirmed the mechanism and the placement, by independent routes. Dev traced the full retention chain for a six-session scope with one session hung, `callbacks` to closures to pending promise to reaction record to resolve-element function to the values list to five sibling snapshots, all reachable and never released under the aggregate form, and all released at the deadline under the per-element one. Grinch reached the same conclusion and added that `Promise.all` has already installed rejection handlers on every element, so later rejections of other lost elements in the same round are handled. Both also confirmed that no invariant and no test changes: painting stays aggregate-only and counter-guarded, rounds stay chained, T1 to T3 pin `initialSessionId="s1"` so N is 1 and every call count and clock step in T1 is unchanged, T4 and T5 test the helper in isolation, T6 does not involve the deadline, T7 compares two constants. The behavioural argument is in Section 5.1 C, the unhandled-rejection consequence for siblings is in Section 4.3, and criterion 9.3.7 exists so a later reviewer cannot quietly collapse it back to one wrapper.

| # | Finding | Resolution |
| --- | --- | --- |
| 2 | Both halves claim false; demonstration cannot fail where it says | **Upheld.** See 14.2 and 12.2.1 above. D is kept as hardening with the reframing the finding asks for, and grinch's option (b), two mutations, is adopted over dev's single revert. |
| 3 | T3 masked by `requestCounter` | **Upheld, against Section 12.2.2.** The deferred is now resolved immediately after the timeout, while round 2 is still the newest request, so the counter cannot supply the answer. A test that passes for two independent reasons cannot demonstrate either one. |
| 4 | The stacking refutation, T7 and the two mechanical errors | **Upheld.** 8 s stays, T7 stays, both reframed as a service level. T5 to T7 corrected, and the prose now matches the test on `<` versus `<=`, with the reason each relation has the strictness it has. |
| 5 | Mount exclusion holds but #1196 is not linked | **Applied.** #1196 is linked in Sections 3 and 10.1, verified OPEN, with the note that no new issue should be filed. |

Also adopted from Section 13.3, though raised only in passing: T5 attaches its `.rejects` expectation before advancing the clock, so Vitest never sees a momentarily unhandled rejection.

### 14.5 What is certified, and what is not

Certified: the design, every coordinate re-verified at `f08b8241` by Step 5 and spot-checked by me, the test specification, and the acceptance criteria.

**Not certified, because nobody has run it:** T1 through T7 do not exist yet. Neither enrichment step was allowed to touch the repository, so their feasibility rests on the harness, on promise semantics executed in isolation, and on the precondition assertion in 9.1 that turns a mount which did not settle into a loud failure. Two specific things the implementer should expect to discover rather than assume:

- **The 50-iteration flush is a margin, not a measurement.** If the precondition assertion fires, raise the count; do not reach for `waitFor`.
- **The residue in Section 4.3 is calculated, not measured.** No native soak was run and none is required to land this. The budget is written down so that it can be checked later and so that being wrong about it is visible.

Neither of these blocks implementation, and neither is a decision left open: in both cases the plan says what to do if reality differs.

### 14.6 Round 2: what the review of the per-element placement changed

The placement itself was confirmed by both enrichers and does not change. What round 2 found was an error in the accounting around it and an overclaim in how the result was presented. Four things changed; none of them touches the design.

1. **The rate was `N`, and it is `N + 1` under a transport-wide outage.** Every round fires `settingsStore.refresh()` from the `finally`, whose `get_settings` is just as lost as the activity calls when the transport is broken. Single-session over eight focused hours is therefore about **3,200** lost calls, not 1,600. Section 4.3 now carries both rows, since the two failure modes genuinely differ: a fault confined to `get_watcher_activity` costs `N`, a transport-wide one costs `N + 1`. I missed this at round 1 by counting only the calls the plan edits.

2. **The `+1` is accepted, not bounded, and the reason is now written down in three places.** This was the open design question round 2 raised, and it resolves cleanly against bounding, decisively on the first of three grounds. `settingsStore.refresh()` returns `void`, not a promise (`settings.ts:28-32`), so there is nothing at the call site for `withDeadline` to race; reaching it means editing a store every window shares, which Section 5.3 excludes and Section 10.4 covers properly. Even granting the promise, a deadline there would release **no** memory: what the deadline buys inside `refresh()` is the release of payload held by a waiting aggregate, and a lone `get_settings` whose result is discarded has no aggregate and no siblings. And gating the call on a successful round, the third option, only helps in the transport-wide case where every `invoke` in the application is already leaking, while costing real behaviour in the command-specific case where `get_settings` still works and labels should keep updating. Recorded as a decision row in Section 4.2, in Section 4.3, and in Section 10.2, which had described that call as harmless without noting that it now contributes here.

3. **"About 1 KB per lost call" is no longer stated as an acceptance threshold.** The **shape** of the residue is verified and remains the load-bearing, checkable claim: two callback entries, their closures, one `Error`, and no match payload of any kind. The **byte figure** is an unmeasured estimate and cannot be verified by reading source, since object sizes, `Error` stack cost and any native-side queued state are out of reach while the cause of the lost reply is unknown. Section 4.3 now separates the two and says which one falsifies the design if it turns out wrong. It is the shape, not the number.

4. **Three small corrections from Step 5's reviewer, folded in.** Section 5.1 C no longer says an empty scope "still" arms nothing, since the rejected aggregate form did arm one and the word implied continuity that did not exist. Criterion 9.3.7 now says explicitly that the check is by eye and warns against mechanising it, because `withDeadline(` and `PtyAPI.getWatcherActivity(` sit on different lines, so a single-line `grep` matches nothing and would read as a pass or a failure purely by accident of phrasing. And Section 12.2.2's edge-case-8 bullet now carries a `[Superseded]` marker, which evens up an asymmetry worth naming: at round 1 I marked the superseded bullet in grinch's Section 13.3 and left the equivalent one in dev's Section 12.2.2 unmarked.

Round 2 did not reopen anything from 14.2 to 14.5, and nothing in it required a new decision beyond item 2.

### 14.7 Round 3: criterion 9.3.6 contradicted Section 5.1 D, and the criterion was the wrong one

Raised by the implementer during Step 8 and escalated rather than settled at the keyboard, which was correct: the plan demanded two things that cannot both hold, and choosing between them is a certification decision, not an implementation one.

**The contradiction.** Section 5.1 D mandates `runPollRound` verbatim, comment included, and that comment contains the literal string `refresh().then()`. Criterion 9.3.6 required `grep -n "refresh().then" src/watchers/App.tsx` to return nothing. Write the code the plan specifies and the grep returns one line; drive the grep to zero and the mandated comment has been edited. No implementer action satisfies both.

**Resolved in favour of the code. Section 5.1 D does not change, the implementation does not change, and only criterion 9.3.6 is rewritten.** Three reasons, heaviest first:

1. **The comment is right and the criterion was wrong.** Naming the shape being replaced is the entire purpose of that comment; it is what stops a later reader from putting the re-arm back inside a `.then()`. A criterion is an instrument for measuring the code. When instrument and subject disagree and the subject is correct, the instrument is what gets repaired.

2. **This is the failure mode the plan already caught once, one criterion later.** Section 14.6 item 4 demoted 9.3.7 to an explicit by-eye check because a single-line pattern does not survive the real code layout and so "lies when it is run". 9.3.6 carries the identical defect from the opposite side: 9.3.7 would have returned zero and read as a pass, 9.3.6 returns one and reads as a failure, and in neither case does the number speak to the property. One was caught and the other missed in the same pass, which is worth recording as a habit to check for rather than a one-off slip: every criterion in this plan that is phrased as a pattern match was written before the code it matches existed.

3. **Rewording the comment would change code that is implemented and green, and would buy nothing.** The routing question is answered by that: **this resolution does not change the code.**

**What replaces it, and why it is stronger than what it replaces.** The new 9.3.6 greps `\.then(` instead of `refresh().then`, and requires exactly one match which must be the mandated comment. Verified independently in the working tree before writing this, not taken on report: at HEAD `6a96b4cebd32159e1eb01b6c67a18ee45d74d1c9`, `grep -n "\.then(" src/watchers/App.tsx` returns the single line `:440`, the `#1188` doc comment, and `git grep -n "\.then(" f08b8241 -- src/watchers/App.tsx` returns the single line `:387`, `void refresh().then(() => {`. The count is one on both sides, so what carries the criterion is the identity of the match and not the arithmetic: executable code before, prose after. The wider pattern also closes a hole the old one had, since `refresh().then` would have missed a re-arm relocated onto any other promise, and it leaves `withDeadline`'s `.finally(` alone.

**What the mechanised half proves and what it does not.** The count is deterministic and can be automated. Deciding honestly that the one surviving match is prose cannot be, not in a single-line pattern, so the criterion says to read it. That is 9.3.7's rule applied consistently: mechanise what survives mechanisation, and say plainly where a human has to look.

**Nothing else changes.** No design, no test, no other criterion, no source file. Section 12.1's 9.3.6 row keeps its baseline observation, which was correct and re-verified, and now carries a `[Superseded]` marker on the "1 to 0" conclusion alone. Rounds 1 and 2 are not reopened, and no finding from Sections 12 or 13 is disturbed.

