# Plan #1581: container shutdown worker pool reclaim after deadline overrun

- Issue: https://github.com/mblua/AgentsCommander/issues/1581 (part of #1578)
- Branch: `fix/1581-container-worker-pool-reclaim`, from `main` `ca6744ae`
- Planning evidence frozen at: `ca6744ae` (all line numbers below)
- Round 4. Score 39, Lite band. `PARTITION: 1 phase`. Class: design-bearing. Owner: dev-rust.
- Task class: routine application code. Threat model: none elevated. Enhanced controls: none apply (no release, signing, migration, security boundary or custom runner).
- Status: READY_FOR_IMPLEMENTATION
- Unqualified `:NNNN` means `src-tauri/src/pty/container_backend.rs`. Other files are named.

## 1. Problem (verified at ca6744ae)

- A worker (`container_shutdown_worker`, `:1252-1300`) waits in `work_available.wait(state)` (`:1272-1274`) with no timeout. It exits only when `state.terminating` is true (`:1279-1287`).
- `terminating = true` is written only by the drain at `:1075` (only when all work is drained) and by `Drop` at `:1227`. `:900` resets it.
- If the deadline passes while work is still active, the drain loop stops (`:1084-1087`) and never marks termination. Idle workers wait forever while the registry lives. `worker_count` stays at 4.
- If the drain cannot take the state lock before the deadline (`begin_shutdown` `:1042-1048`, loop `:1067`), nothing marks termination either.
- `ensure_workers` overwrites `worker_count` with the handle count (`:1033-1037`). This is the only writer that raises it. A worker that already decremented but whose thread has not finished is still counted.
- `Drop` (`:1214-1249`) moves every unfinished handle into `PROCESS_RETAINED_CONTAINER_WORKERS` (`:1210`), including idle workers that were just woken. Those handles are never joined.

## 2. Decision

The worker decides its own exit from the deadline and the producer count, under the state lock it already takes to decrement. It does not wait for the drain to mark termination, so the drain's lock is off the exit path.

Late work keeps running (§3.5). Every pre-seal admission (`spawn_producer_owned`, `:738`) goes through a live `ContainerShutdownProducer` (`:1321-1326`; `register_producer` `:723-736` only in `Accepting`). So while `active_producers > 0`, late work can still arrive, and workers stay. When it reaches 0 after the deadline, no pre-seal work can arrive, and workers exit.

`Drop` joins idle workers within a bounded grace. It retains only workers still running an item.

Rejected: marking `terminating` in the drain loop (it still needs the drain to get the lock). Rejected: a longer budget or a weaker assertion (the issue forbids both). Rejected (round 2 design): exiting at the deadline regardless of producers. Late producer work then lands in `retained` and may never run before a sweep. `assert_real_pending_container_start_shutdown_waits_for_stop` (`session/selection.rs:3843`) waits for `shutdown_work_state_for_test() == (true, 0, 0)` at `selection.rs:4139-4141`, before its sweep at `:4179`. Stranded retained work would fail the two named tests.

## 3. Contract

### 3.1 State

Add `sweep_deadline: Option<Instant>` to `ContainerShutdownWorkState` (`:664-676`).

`start_global_sweep_epoch` (`:892-909`) sets `state.sweep_deadline = Some(deadline)` in the same critical section where it sets `phase = GlobalSweep` and `terminating = false`.

Why a separate field: the control deadline is sticky `min(existing, new)` (`container_runtime.rs:67-76`). After a drain overrun it stays expired. A later epoch with a fresh deadline would otherwise make workers exit before sweep work arrives.

### 3.2 Worker exit (the fix)

In the inner loop of `container_shutdown_worker`, keep the order: take queued work, then retained work, then check `terminating` (exit). After that, still holding the state lock:

1. Effective deadline: `state.sweep_deadline` when `phase == GlobalSweep`, otherwise `shared.control.shutdown_deadline()`.
2. If it is `Some(d)`, `Instant::now() >= d` and `state.active_producers == 0`: if `phase == Accepting`, set `phase = Draining` (the seal a failed `begin_shutdown` could not write, `:1042-1048`); set `state.terminating = true`; `notify_all` on `work_available` and `state_changed`; exit (decrement below).
3. If it is `Some(d)` and `d` has not passed: `work_available.wait_timeout(state, d.saturating_duration_since(now))`, then loop.
4. Otherwise (no deadline, or expired with producers alive): `work_available.wait_timeout(state, CONTAINER_SHUTDOWN_IDLE_RECHECK)` (new const, 1 s), then loop.

No condvar wait in the worker is unbounded.

Exit decrement is atomic: both exits (the `terminating` check and step 2) do `worker_count -= 1` (underflow guard, log) and `state_changed.notify_all()` inside the inner-loop critical section before `break None`. The separate re-lock block (`:1279-1287`) is removed; the outer `None` arm only returns. So `terminating == true` is never observable with a stale count from the worker that set it.

Why step 2 seals `Accepting`: the control deadline on `shared.control` is set only by `begin_shutdown` (`:1041`; other `request_shutdown` callers use other controls, V4). If its state lock times out, phase stays `Accepting`. Without the seal, `register_producer` (`:723-736`) would admit a producer whose work is refused (`:758`, `terminating`) and retained (`:838`) until the sweep. With it, that producer is refused, as after a successful `begin_shutdown`. Disclosed residual: while producers from before the failed seal are alive (rule 4), new producers can still register in `Accepting`; that is pre-fix behaviour.

Producer `Drop` (`:1329-1346`): where it reaches `active_producers == 0` (`:1342-1344`), also `work_available.notify_all()` under the state lock it holds. Idle workers then exit promptly instead of at the next recheck.

Why rule 4 has a timeout: the deadline lives under the control lock, the condvar under the state lock. `begin_shutdown` sets the deadline (`:1041`) and, when the state lock is unavailable, notifies at `:1043-1044` without holding it. A worker that read `None` under the state lock and has not yet entered the wait misses that notify. The recheck bounds the miss at 1 s. Cost: at most 4 wakeups per second, only while workers exist. Rejected: mirroring the deadline into state, because the lock-failure branch cannot write state. The other notifies (`:1053`, `Drop` `:1228`, producer `Drop`) run under the state lock and cannot be missed. `request_shutdown` at `:2750`, `:4699` and in `docker_runtime.rs` runs on other controls (V4 re-runs this census).

A lowered control deadline (sticky min) can leave a worker in rule 3 until the older `d`. That is bounded by `d`.

Lock order: state, then control. `run_producer_fallback` already uses it (`:822`). `container_runtime.rs:67-117` takes only its own lock.

`terminating` is set only at step 2, when no producer is alive, so no pre-seal admission (`:758`) can follow it. Queued or retained items are taken before step 2 under the same lock, so none is stranded. Sweep admission (`:947`) after step 2 needs a new epoch, which resets `terminating`.

The drain loop (`:1066-1088`) stays unchanged. The bounded join block (`:1090-1162`) changes only at its join `Err` (`:1095`, §3.3).

### 3.3 Worker count

`worker_count` counts live logical workers. It is never overwritten:

- `ensure_workers` (`:965-1038`): keep the reap of finished handles (`:970-984`). Replace the `while` loop (`:989`) with a one-shot bound. After the reap, under the state lock once: `deficit = if state.terminating { 0 } else { CAPACITY.saturating_sub(state.worker_count) }`. Then `for _ in 0..deficit`. Before each `spawn`, under the state lock, `worker_count += 1`. On `spawn` `Err`, `-= 1` (underflow guard) and break. The injected `fail_worker_spawn` break (`:990-1007`) stays before the increment.
- Why one-shot plus the `terminating` skip (round-3 blocker): a condition re-read each iteration races workers that decrement concurrently. A replacement spawned while `terminating` is true exits at once, so the loop could spin under the `workers` mutex (reachable: failed `begin_shutdown`, then a producer's `ensure_workers` at `:749`). Now each call spawns at most `CAPACITY` threads, and none while `terminating` (they would only exit). If `terminating` becomes true after `deficit` is read, the extra spawns exit and decrement; the call still ends. Callers after a reset (`start_global_sweep_epoch` `:900` then `:908`) see `terminating == false` and spawn.
- Why the condition changes: a worker that decremented but has not finished still has an unfinished handle. With `workers.len()`, an epoch reopened in that window spawns nothing, and new work has no live worker. With the logical count it spawns a replacement. `workers` may briefly hold more than 4 handles; later calls reap them.
- Remove the overwrite (`:1033-1037`). Return `state.worker_count` read under the state lock, so callers (`:749`, `:908`, `:940`) still get "live workers, 0 means none".
- The worker exit path keeps its decrement (`:1279-1287`).
- Where a join of a `self.workers` handle returns `Err` (a panicked thread that never decremented: `:973`, `:1095`, `:1240`, and the §3.4 helper), decrement with the underflow guard and log.
- The §3.4 step 1 reap joins handles from the process-global static (`:1210`). Those can belong to earlier registries, so a join `Err` there only logs and never touches `worker_count`.

### 3.4 Drop

After the existing block (`:1218-1230`), replace the drain-to-static loop (`:1231-1248`) with:

1. Take the static lock. Join and remove every handle in it that `is_finished()`. A join `Err` logs only.
2. Until `grace = Instant::now() + CONTAINER_SHUTDOWN_DROP_GRACE` (new const, 1 s, same as the `:1217` request): join finished handles from `self.workers`. Read `busy = state.active.len().saturating_sub(state.active_fallbacks)` under the state lock (`lock_mutex_until(grace)`, stop if `None`). Stop when the remaining handles are `<= busy`. Otherwise sleep `CONTAINER_SHUTDOWN_POLL`.
3. Push the remaining handles into the static and log how many.

Put step 2 in a private helper `fn join_idle_workers_until(&self, grace: Instant) -> Vec<std::thread::JoinHandle<()>>` that locks `self.workers` and returns the unfinished handles. `Drop` calls it and does step 3. `&self` lets a test call it through an `Arc`.

`Drop` sets `terminating` (`:1227`), so workers exit at their next check, whatever the producer count (a producer holds an `Arc` to the registry, so none is alive in `Drop`).

### 3.5 Behaviour decisions and out of scope

- **Late producer work (decided: unchanged).** Pre-fix, after an overrun, `terminating` stays false, so a live producer's late work is queued (`:758`) and run by parked workers (`:1262-1275`). After the fix, workers stay while `active_producers > 0` (rule 4), `terminating` stays false, and the same path queues and runs it. The only change: once the last producer drops after the deadline, workers exit.
- `session/selection.rs` (including `spawn_blocking` at `:2835`) and the test wait/assert mismatch (#1582): out of scope.
- `selection.rs:4139-4157` samples `snapshot()` (no worker count) then asserts count 0. The atomic decrement (§3.2) removes the window for the worker that sets `terminating`; the other idle workers still need a wakeup after `(true, 0, 0)` is visible. That residual window belongs to #1582 (wait/assert mismatch), out of scope.
- A worker blocked inside a work item past its deadline stays retained in `Drop`. It is bounded by the item's cooperative `control`, not by this pool.
- Worker exit after an overrun is bounded by the lifetime of the last pre-seal producer, which is the owner of any late work. The issue's named tests show producers at 0 at the overrun (`producers=0 ... active=1 workers=4`).

## 4. Tests (all in `container_backend.rs` `mod tests`, `:3539`)

Every wait is bounded (`recv_timeout` / poll with a 5 s cap). The registry stays alive until the final assertion. T1, T2 and T3 each assert `worker_count() == 4` after spawn and before the deadline. Without it, a tree where nothing raises the count passes them vacuously.

- **T1 drain overrun, busy worker (primary detector).** `Arc<Registry>`. Register a producer; `spawn_owned` blocks on a release channel. Wait for the "started" signal. Assert `worker_count() == 4`. `seal_and_drain_until(now + 100 ms)`: assert `!report.terminal`. Release the work. Drop the producer. Poll `worker_count() == 0` for up to 5 s, then assert `snapshot() == (true, 0, 0)`.
- **T1b producer keeps workers (rule 2 guard).** As T1, but keep the producer alive after the release. Sample `worker_count()` every 10 ms for 200 ms: 4 at every sample. Then `spawn_owned` a quick item that sets a flag: the flag is set within 5 s (late work runs). Drop the producer, poll count `== 0`.
- **T2 state lock held across the deadline (residual detector).** Spawn workers with a quick item via a producer, drop the producer, wait until `snapshot().2 == 0`. Assert `worker_count() == 4`. The test thread takes `registry.shared.state`. Another thread runs `seal_and_drain_until(now + 50 ms)`; join it and assert `!terminal`. Release the lock. Poll `worker_count() == 0` for up to 5 s. Then assert `snapshot().0` (sealed by step 2) and `register_producer().is_none()` (Accepting-hole guard).
- **T3 sweep epoch overrun.** Fresh registry, `begin_shutdown(now + 1 s)`, `start_global_sweep_epoch(now + 100 ms)`, `spawn_global_sweep_owned_with_control` blocking work, assert `worker_count() == 4`, `seal_and_drain_until` with the same deadline, assert `!terminal`, release, poll count `== 0`.
- **T4 epoch reopen after overrun (regression guard).** After T1 (control deadline expired, workers exited): `start_global_sweep_epoch(now + 1 s)`. Sample `worker_count()` every 10 ms for 200 ms: 4 at every sample. Workers that used the expired control deadline instead of `sweep_deadline` exit within milliseconds, so this fails deterministically. Then a quick sweep item that sets a flag, `seal_and_drain_until(same)`: assert `terminal`, flag set, count `== 0`.
- **T5 Drop helper.** Registry with 4 workers, one running an item blocked on a release channel. Under the state lock set `terminating = true` and `notify_all`, as `Drop` does. `join_idle_workers_until(now + 5 s)` returns exactly 1 handle (the helper returns as soon as remaining `<= busy`, so the wider test grace costs nothing on a fast run and absorbs CI load; `Drop` still passes 1 s). Release the item and join that handle. Contract test, not a pre-fix detector.
- **T7 no spawn while terminating (round-3 blocker guard).** Registry with 4 workers. Under the state lock set `terminating = true`, `notify_all`. Poll `worker_count() == 0` (5 s). Run `ensure_workers` on a spawned thread that sends its return value; `recv_timeout(5 s)` gets `0`. Then `workers.lock().len()` after reaping is `0`, and `worker_count() == 0`. With a re-read `while worker_count < CAPACITY` loop and no `terminating` skip this can time out; with the fix it returns at once.
- **T6 reopen right after exit (sanity, probabilistic; say so in a comment).** 50 iterations: overrun as in T2 without holding the lock, then immediately `start_global_sweep_epoch(now + 1 s)` + quick sweep item + `seal_and_drain_until(same)`: assert terminal and count `== 0`.

Existing tests stay unchanged and must pass: `container_backend.rs` `blocked_fallback...` (`:5055-5097`), and in `session/selection.rs` `:4157`, `:5940`, `:5971` (each asserts `shutdown_worker_count_for_test() == 0`) and the two named tests `:5821`, `:5831`.

## 5. Dependencies and layering

Changes stay inside `pty/container_backend.rs` and use only `std` items already in use (`Instant`, `Condvar::wait_timeout`, `std::thread::JoinHandle`). No new cross-module reference: no new arc, the SCC is unchanged. The reference to `ContainerRuntimeControl::shutdown_deadline` already exists (`:822`).

## 6. Delivery (dev-rust implements, tech-lead delivers the PR)

Preconditions (V0), run in `repo-AgentsCommander`:
```
git rev-parse --show-toplevel         # ends with repo-AgentsCommander
git branch --show-current             # fix/1581-container-worker-pool-reclaim
git fetch origin main && git merge-base --is-ancestor ca6744ae origin/main
git status --porcelain                # empty
git diff --name-only ca6744ae origin/main -- src-tauri/src/pty src-tauri/src/session/selection.rs
```
If `container_backend.rs`, `container_runtime.rs` or `selection.rs` changed since `ca6744ae`, re-anchor §1-§4 before editing. Other drift: record it and continue.

Validation (cwd `src-tauri`, toolchain = CI's stable, `--locked`):
- V1 `cargo fmt --all -- --check`. `git diff --name-only ca6744ae` lists only `src-tauri/src/pty/container_backend.rs` and `plans/1581-container-worker-pool-reclaim.md`.
- V2 `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- V3 differential proof, `cargo test --locked --lib pty::container_backend::tests::<name>`, record output:
  (a) Pre-fix tree plus T1, T2, T3: each red at the 5 s poll, with its pre-deadline `== 4` assert passing.
  (b) Control: (a) plus §3.3 only (no §3.1, §3.2, §3.4, producer-`Drop` notify): T1-T3 still red at the 5 s poll, pre-deadline assert passing. This proves the counting change alone lowers nothing and §3.2 turns them green. If the pre-deadline assert fails here, §3.3 is wrong: stop.
  (c) Full change: T1-T7 green.
  (d) (c) with rule 2 ignoring `active_producers`: T1b red. (c) with step 1 using only the control deadline: T4 red at the 200 ms sample. (c) with step 2 not sealing `Accepting`: T2 red at `register_producer().is_none()`. (c) with `ensure_workers` re-reading `worker_count` each iteration and no `terminating` skip: T7 red or timed out (record which; it is racy, so a green run here is recorded, not a stop).
  Restore after each control. Commit tests and fix together. Paste the red and green excerpts into the PR body.
- V4 census: `git grep -n "request_shutdown(" src-tauri/src` and confirm §3.2's list. `git grep -n "worker_count" src-tauri/src/pty/container_backend.rs` shows no assignment other than `+= 1` / `-= 1`, and no `while` loop conditioned on it in `ensure_workers`. `git grep -n "\.wait(" src-tauri/src/pty/container_backend.rs` shows none inside `container_shutdown_worker`.
- V5 `cargo test --locked --lib pty::container_backend::tests` and `cargo test --locked --lib session::selection::tests` 20 times in a loop (bounded by `timeout 1800`). Report the failure count. Supporting evidence only (issue: 240 runs did not reproduce the named tests).
- V6 `cargo test --locked --lib --bins --tests` once (the CI step `pr-regression-gates.yml:94-96`).

Environment risk (dev states it in writing): Defender real-time protection is off locally. Windows CI load cannot be reproduced locally. T1-T4 are deterministic and do not depend on load.

PR: into `main`, body `Fixes #1581` only (not #1578). Every triggered and required check green on the exact PR head SHA. A failure on another SHA or an unexplained skip does not count. On failure: revert only this branch's own changes to `container_backend.rs`. No broad reset.

## 7. Acceptance

1. V3 (a), (b) and (d) red as stated; (c) green. Evidence in the PR.
2. No unbounded condvar wait remains in `container_shutdown_worker` (V4).
3. `worker_count` is never assigned from `workers.len()` (V4).
4. Late producer work still runs after an overrun (T1b). Workers exit once producers are gone (T1).
5. `Drop` retains only busy workers (T5). The static is reaped.
6. V1, V2, V5, V6 and exact-head CI green. Diff limited to the 2 paths in V1.
7. `ensure_workers` spawns at most `CAPACITY` per call and none while `terminating` (T7). A failed `begin_shutdown` is sealed by the exiting worker (T2).
