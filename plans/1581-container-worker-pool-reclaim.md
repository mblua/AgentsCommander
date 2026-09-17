# Plan #1581: container shutdown worker pool reclaim after deadline overrun

- Issue: https://github.com/mblua/AgentsCommander/issues/1581 (part of #1578)
- Branch: `fix/1581-container-worker-pool-reclaim`, from `main` `ca6744ae`
- Planning evidence frozen at: `ca6744ae` (all line numbers below)
- Score 39, Lite band. `PARTITION: 1 phase`. Class: design-bearing. Owner: dev-rust.
- Task class: routine application code. Threat model: none elevated. Enhanced controls: none apply (no release, signing, migration, security boundary or custom runner).

## 1. Problem (verified at ca6744ae)

File: `src-tauri/src/pty/container_backend.rs`.

- A worker (`container_shutdown_worker`, `:1252-1300`) waits in `work_available.wait(state)` (`:1272-1274`) with no timeout. It exits only when `state.terminating` is true (`:1279-1287`).
- `terminating = true` is written only by the drain at `:1075` (only when all work is drained) and by `Drop` at `:1227`. `:900` resets it.
- If the deadline passes while work is still active, the drain loop stops (`:1084-1087`) and never marks termination. Idle workers wait forever while the registry lives. `worker_count` stays at 4.
- If the drain cannot take the state lock before the deadline (`begin_shutdown` `:1042-1048`, loop `:1067`), nothing marks termination either. This is the residual left open by proposal v2 §3.4.
- `ensure_workers` overwrites `worker_count` with the handle count (`:1033-1037`). A worker that already decremented but whose thread has not yet finished is still counted, and nothing decrements it again. After an overrun this can leave the count above 0.
- `Drop` (`:1214-1249`) moves every unfinished handle into `PROCESS_RETAINED_CONTAINER_WORKERS` (`:1210`) at once, including idle workers that were just woken. Those handles are never joined.

## 2. Decision

The worker decides its own exit from the deadline. It does not wait for the drain to mark termination. This removes the two residuals from proposal v2:

- **Lock unavailable until `Drop`:** gone. The drain's lock is no longer on the exit path. The worker exits under its own lock acquisition, which it needs anyway to decrement the count. Every state-lock holder runs a short critical section with no blocking call inside (the condvar waits release the lock).
- **Retained handles never joined:** idle workers are now joined in `Drop` within a bounded grace period. Earlier leftovers in the static are reaped on each `Drop`. A handle is retained only for a worker still running a work item that has overrun its deadline. It cannot be joined without an unbounded wait, which the issue forbids. That is not the routine overrun case.

Rejected: marking `terminating` in the drain loop (proposal v2). It still depends on the drain getting the lock. Rejected: a longer budget or a weaker assertion (the issue forbids both). Rejected: a periodic wake tick with no deadline. It would wake 4 threads for the whole app lifetime, and every path that sets a deadline already notifies (§3.2).

## 3. Contract

### 3.1 State

Add `sweep_deadline: Option<Instant>` to `ContainerShutdownWorkState` (`:664-676`).

`start_global_sweep_epoch` (`:892-909`) sets `state.sweep_deadline = Some(deadline)` in the same critical section where it sets `phase = GlobalSweep` and `terminating = false`.

Why a separate field: the shared control deadline is `min(existing, new)` (`container_runtime.rs:73-76`). After a drain overrun it stays at the old, expired value. A sweep epoch opened later with a fresh deadline would otherwise make workers exit before sweep work arrives.

### 3.2 Worker exit (the fix)

In the inner loop of `container_shutdown_worker`, keep the order: take queued work, then retained work, then check `terminating`. After that, and while still holding the state lock:

1. Effective deadline: `state.sweep_deadline` when `phase == GlobalSweep`, otherwise `shared.control.shutdown_deadline()`.
2. If the deadline is `Some(d)` and `Instant::now() >= d`: set `state.terminating = true`, `notify_all` on `work_available` and `state_changed`, and exit through the existing decrement path.
3. If the deadline is `Some(d)` and has not passed: `work_available.wait_timeout(state, d - now)`, then loop again.
4. If it is `None`: keep the current unbounded `wait`. This is reachable only before any shutdown request (phase `Accepting`, no control deadline). It is not after an overrun.

Why rule 4 is safe: `shared.control` receives a deadline in only two places. `begin_shutdown` (`:1041`) notifies `work_available` on both its success and lock-failure branches (`:1043`, `:1053`). `Drop` (`:1217`) notifies at `:1228`. `request_shutdown` at `:2750`, `:4699` and in `docker_runtime.rs` runs on other controls. The implementer re-runs this census (§6, V4).

Lock order: state, then control. `run_producer_fallback` already uses this order (`:822`). No code takes the control lock and then the state lock (`container_runtime.rs:67-117` takes only its own lock).

Setting `terminating` (not only exiting) is deliberate. It closes queue admission (`:758`, `:947`), so late work goes to the existing fallback. After the deadline that fallback puts the work in `retained` (`:822-846`). `stop_all_started_containers_blocking` (`:2803-2821`) sees retained work and reopens an epoch, which resets `terminating` and spawns workers again. No work is lost. Termination does not mean quiescence: a producer registered before the seal can still respawn workers for a short time. These workers pick up retained work first, then exit on their own.

The drain loop (`:1066-1088`) and the snapshot (`:1090-1162`) stay unchanged. The report keeps its current meaning.

### 3.3 Worker count

`worker_count` counts live logical workers. It is never overwritten:

- In `ensure_workers` (spawn loop `:989-1031`), before each `spawn`: `state.worker_count += 1` under the state lock. On `spawn` `Err`: `-= 1`. The injected `fail_worker_spawn` break (`:990-1007`) happens before the increment.
- Remove the overwrite at `:1033-1037`. The return value stays `workers.len()`, and its callers are unchanged.
- The worker exit path keeps its decrement (`:1279-1287`).
- Where a join of a `self.workers` handle returns `Err` (a panicked thread that never decremented: `:973`, `:1095`, `:1240`, and the §3.4 step 2 helper), decrement with the existing underflow guard and log.
- The §3.4 step 1 reap joins handles from the process-global static (`:1210`). Those may belong to earlier dropped registries, so a join `Err` there only logs. It never touches `worker_count`. A decrement there could undercount live workers and make `drained` (`:1078`) or `has_owned_work_until` (`:917`) report a false terminal.

### 3.4 Drop

After the existing block (`:1218-1230`), replace the drain-to-static loop (`:1231-1248`) with:

1. Take the static lock. Join and remove every handle in it that `is_finished()`. This is the reap of leftovers from earlier drops. A join `Err` here logs only (§3.3).
2. Until `grace = Instant::now() + CONTAINER_SHUTDOWN_DROP_GRACE` (new const, 1 s, the same value as the `:1217` request): join finished handles from `self.workers`. Read `busy = state.active.len() - state.active_fallbacks` under the state lock (`lock_mutex_until(grace)`, stop if `None`). Stop when the remaining handles are `<= busy`. Otherwise sleep `CONTAINER_SHUTDOWN_POLL`.
3. Push the remaining handles into the static and log how many.

Put step 2 in a private helper `fn join_idle_workers_until(&self, grace: Instant) -> Vec<JoinHandle<()>>` that locks `self.workers` and returns the unfinished handles. `Drop` calls it and does step 3. Taking `&self` lets a test call it through an `Arc`.

Drop is bounded by the grace period. It never waits on a busy worker. Idle workers exit within milliseconds of the `:1228` notify.

### 3.5 Out of scope, unchanged

- `session/selection.rs` (including the retained `spawn_blocking` task at `:2848-2858`) and the test wait/assert mismatch (#1582).
- `ensure_workers` still ignores `terminating`. Reopening an epoch still works as before.
- A worker blocked inside a work item past its deadline stays retained. This is bounded by the work's own cooperative `control`, not by this pool.
- Trade-off: work that arrives after the deadline lands in `retained` (§3.2). Session close (`session/selection.rs:2836`) calls only `seal_and_drain_shutdown_work_blocking` and never reopens an epoch, so it reports that work as retained and does not run it. Only the global sweep (`lib.rs:4087`, `stop_all_started_containers_blocking`) reopens and runs it. This is the pre-fix behaviour for post-deadline work; the fix does not add a reopen to session close.

## 4. Tests (all in `container_backend.rs` `mod tests`, `:3539`)

Every wait is bounded (`recv_timeout` / poll with a 5 s cap). The registry stays alive until the final assertion.

- **T1 drain overrun, busy worker (primary detector).** `Arc<Registry>`. Producer `spawn_owned` blocks on a release channel. Wait for the "started" signal. `seal_and_drain_until(now + 100 ms)`: assert `!report.terminal`. Release the work. Poll `worker_count() == 0` for up to 5 s. Drop the producer (its guard and join the owned work) so `active_producers == 0`, then assert `snapshot() == (true, 0, 0)`. Before the fix: stays at 4 for the whole 5 s, deterministic red (nothing can set `terminating` while the registry lives and no drain runs). After the fix: 0.
- **T2 state lock held across the deadline (residual detector).** Spawn workers with a quick work item and wait until `snapshot().2 == 0`. The test thread takes `registry.shared.state` lock. Another thread runs `seal_and_drain_until(now + 50 ms)`. Join it and assert `!terminal`, then release the lock. Poll `worker_count() == 0` for up to 5 s. Before the fix: red (the `begin_shutdown` lock fails, nothing marks termination). After the fix: green (the notify at `:1043` plus the control deadline).
- **T3 sweep epoch overrun.** After T1-style exhaustion (or on a fresh registry with `begin_shutdown`): `start_global_sweep_epoch(now + 100 ms)`, `spawn_global_sweep_owned_with_control` blocking work, `seal_and_drain_until` with the same deadline, assert `!terminal`, release, poll count `== 0`. Covers `sweep_deadline`. Before the fix: red.
- **T4 epoch reopen still runs work (regression guard).** After an overrun in which workers exited: `start_global_sweep_epoch(now + 1 s)`, a quick sweep work item that sets a flag, `seal_and_drain_until(same)`: assert `terminal`, the flag is set, and the count `== 0`. Proves `sweep_deadline` keeps expired control deadlines from killing the new epoch.
- **T5 Drop helper.** Registry with 4 workers, one of them running an item blocked on a release channel. Under the state lock set `terminating = true` and `notify_all`, as `Drop` does. Call `join_idle_workers_until(now + 1 s)` and assert it returns exactly 1 handle. Release the item and join that handle in the test. Contract test, not a pre-fix detector.
- **T6 count after respawn race (sanity).** 50 iterations: overrun as in T1 with no blocking work, `start_global_sweep_epoch` + `seal_and_drain_until(now + 1 s)`: assert terminal and count `== 0`. Probabilistic only. Label it so in a comment.

Existing tests stay unchanged and must pass, including `blocked_fallback...` (`:5055-5097`) and the selection tests at `:4157`, `:5940`, `:5971`, `:5821`, `:5831`.

## 5. Dependencies and layering

Changes stay inside `pty/container_backend.rs` and use only `std` items already imported (`Instant`, `Condvar::wait_timeout`, `JoinHandle`). No new cross-module reference: no new arc, the SCC is unchanged. The reference to `container_runtime::ContainerRuntimeControl::shutdown_deadline` already exists (`:822`).

## 6. Delivery (dev-rust implements, tech-lead delivers the PR)

Preconditions (V0), run in `repo-AgentsCommander`:
```
git rev-parse --show-toplevel         # ends with repo-AgentsCommander
git branch --show-current             # fix/1581-container-worker-pool-reclaim
git fetch origin main && git merge-base --is-ancestor ca6744ae origin/main
git status --porcelain                # empty except this plan
git diff --name-only origin/main...HEAD -- src-tauri   # re-check drift in container_backend.rs / container_runtime.rs
```
If `container_backend.rs` or `container_runtime.rs` changed since `ca6744ae`, re-anchor §1-§4 before editing. Other drift: record it and continue.

Validation (cwd `src-tauri`, toolchain = CI's stable, `--locked`):
- V1 `cargo fmt --all -- --check` scoped: `git diff --name-only` must list only `src-tauri/src/pty/container_backend.rs` and `plans/1581-container-worker-pool-reclaim.md`.
- V2 `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- V3 pre-fix proof: add T1, T2 and T3 first, run `cargo test --locked --lib pty::container_backend::tests::<name>`, and record red output for each (the 5 s poll panics). Then apply §3 and record green. Positive control (differential): on the pre-fix tree, remove only the `:1033-1037` overwrite and re-run T1-T3; they must stay red, proving the overwrite removal alone cannot decrement and the §3.2 worker exit is what turns them green. Record that output too, then restore. Commit tests and fix together. Paste the red and green excerpts into the PR body for grinch.
- V4 census: `git grep -n "request_shutdown(" src-tauri/src` and confirm each call on `shared.control` notifies `work_available` (§3.2 rule 4). `git grep -n "worker_count" src-tauri/src/pty/container_backend.rs` shows no assignment other than `+= 1` / `-= 1`.
- V5 `cargo test --locked --lib pty::container_backend::tests` and `cargo test --locked --lib session::selection::tests` 20 times in a loop (bounded by `timeout 1800`). Report the failure count. Green here is supporting evidence only, not proof (issue: 240 runs did not reproduce these names).
- V6 `cargo test --locked --lib --bins --tests` once (matches the CI step `pr-regression-gates.yml:94-96`).

Environment risk (dev states it in writing): the local box has Defender real-time protection off. Windows CI load cannot be reproduced locally. T1-T3 are deterministic and do not depend on load.

PR: into `main`, body `Fixes #1581` only (not #1578). Every triggered and required check must be green on the exact PR head SHA. A failure on another SHA or an unexplained skip does not count. On failure: revert only this branch's own changes to `container_backend.rs`. No broad reset.

## 7. Acceptance

1. T1, T2 and T3 red before §3 and green after (V3 evidence in the PR).
2. No unbounded condvar wait is reachable once a control or sweep deadline exists (review §3.2 against the diff).
3. `worker_count` is never assigned from `workers.len()` (V4).
4. `Drop` retains only busy workers (T5). The static is reaped.
5. V1, V2, V5, V6 and exact-head CI green. Diff limited to the 2 paths in V1.
