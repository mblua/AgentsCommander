# Plan #1579: `LockGuard::acquire` retries Windows transient access-denied

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/1579 (OPEN, part of epic #1578)
- Repo: `repo-AgentsCommander`; branch `fix/1579-lock-access-denied-retry`
- Base (frozen 2026-09-16): branch HEAD = `origin/main` = `5203c4e3b0b5909fdc12337194c886b1514966c3`
  (`git rev-parse HEAD origin/main` after `git fetch origin main`). Tracked tree clean. Line numbers
  refer to that SHA; if a quoted line moved, re-anchor on the quoted text.
- Class: Lite (score 34). One phase, no partition. Owner: Rust dev; reviewer: Grinch (proof veto);
  coordinator `ac-tech-lead-v4`.
- Task class and threat model: routine product bug fix, no security boundary, no release. Baseline
  gates only; every enhanced control (binary provenance, OS-handle exclusion, custom runners) is
  not applicable because nothing here builds, signs or migrates anything.
- Files: 1 modified, `src-tauri/src/cli/task_ops.rs` (production + in-file tests), plus this plan
  (`git add -f`, `/plans/` is gitignored). No IPC, wire, lock-file format, dependency or workflow
  change. New code references only `std`: no new module arc, dependency-cycle gate is neutral.

## 1. Verified cause (base 5203c4e3)

`task_ops.rs:434` opens the lock with `create_new`. Only `:447`
`Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists` retries; `:468`
`Err(e) => return Err(TaskOpError::LockIo(path.to_path_buf(), e))` is fatal for everything else.
`Drop` (`:474-478`) does `remove_file`. The rename site (`:657-665`) already retries 5 and 32.

Architect probe, 2026-09-16, this Windows 11 box, rustc 1.97.1, standalone crate in the architect
replica (`scratch/probe1579`, not in the repo):

| Probe | Result |
|---|---|
| 8 threads x 20k raw `create_new`/`remove_file` | ok 16,195; `AlreadyExists`/80 116,074; `PermissionDenied`/**5** 27,731; **303: 0**; nothing else |
| Hold handle, `remove_file`, then `create_new` (deterministic "delete pending") | `create_new` **succeeds** (POSIX delete semantics). This repro cannot detect the bug: the proposal's T8 is dropped |
| `create_new` on an existing directory | `PermissionDenied`/**5** |
| `create_new` under a missing parent | `NotFound`/3 |
| 4 raw hammers + one acquire-shaped loop (50 ms sleep on 80, fatal otherwise), 20 trials debug+release | fatal os 5 in **20/20**, first hit 51 ms to 2.54 s |

Closes proposal §5.3: the race code is 5, not 303. A directory also yields 5, so no code-only
rule can tell them apart.

## 2. Decision

### 2.1 Classifier (new, above `impl LockGuard`, after `:425`)

```rust
/// #1579: on Windows a contended `CREATE_NEW` can fail with ERROR_ACCESS_DENIED (5) while the
/// holder's `remove_file` is in flight, or ERROR_SHARING_VIOLATION (32) while another process
/// holds the file. Both are contention. Raw codes only on Windows: 5 is EIO and 32 is EPIPE on Unix.
#[cfg(windows)]
fn is_transient_lock_open_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
fn is_transient_lock_open_error(_e: &std::io::Error) -> bool {
    false
}
```

Not included: 303 (never observed, see §1). Not keyed on `ErrorKind::PermissionDenied`, so no
other code is widened.

### 2.2 `acquire` (`:427-471`)

1. Before `loop`: `let mut saw_already_exists = false;`
2. First line of the `AlreadyExists` arm: `saw_already_exists = true;`. Rest of the arm unchanged
   (stale check, `LockTimeout`, 50 ms sleep).
3. New arm between `AlreadyExists` and the catch-all:

```rust
Err(e) if is_transient_lock_open_error(&e) => {
    // #1579: contention only if a holder was ever seen. A persistent 5/32 with no
    // AlreadyExists in this call (directory at the path, ACL denial, foreign holder)
    // is a real I/O failure and must not be reported as a timeout.
    if start.elapsed() >= timeout {
        return Err(if saw_already_exists {
            TaskOpError::LockTimeout
        } else {
            TaskOpError::LockIo(path.to_path_buf(), e)
        });
    }
    std::thread::sleep(Duration::from_millis(50));
}
```

4. Catch-all `:468`, `Drop`, `LOCK_TIMEOUT_5S`, `LOCK_STALE_AFTER_5M`, the stale check and
   `concurrent_writes_return_correct_post_edit_content` (`:1387`) are unchanged.

Why this shape:
- Stale detection is not bypassed: the new arm never removes a file. If a lock really exists the
  next attempt gets `AlreadyExists` and runs the unchanged stale check.
- Same deadline and poll; no timeout widened.
- Genuine errors stay `LockIo`. Tradeoff, accepted: a persistent Windows 5/32 (directory at the
  lock path, ACL denial) now reports `LockIo` after the deadline (5 s in production) instead of at
  once. Proposal §5.1.3's immediate directory guard is dropped: the deadline rule already keeps
  it `LockIo`, and T2 can then test it. Residual: if `AlreadyExists` was seen earlier in the same
  call and then only 5/32 until the deadline, the result is `LockTimeout`. Both are errors; only
  the variant differs.
- Non-Windows: the classifier is constant `false`, so the arm never matches. Behavior identical.

## 3. Out of scope

- Rename retry (`:663-665`) tests 5/32 without `cfg(windows)` (32 is EPIPE on Linux). Separate
  ticket; the coordinator decides.
- Switching to `File::try_lock` (as in `config/local_config_io.rs`): protocol change, not needed.
- Softening `concurrent_writes_return_correct_post_edit_content`, or any timeout change.

## 4. Tests (in `mod tests`, after `lock_guard_recovers_stale_lockfile`, which ends at `:955`)

Prefix `issue_1579_` so a filter selects all three.

**T1 `issue_1579_transient_lock_open_error_classification`** (all platforms)
- `from_raw_os_error(5)` and `(32)`: `== cfg!(windows)`.
- `from_raw_os_error(2)`, `(303)`, and `io::Error::from(ErrorKind::AlreadyExists)`: `false`.

**T2 `issue_1579_directory_at_lock_path_is_lock_io_after_deadline`** (`#[cfg(windows)]`)
- `FixtureRoot::new("task-1579-dir")`, `create_dir` at `TASK.md.lock`.
- `acquire(&lock, Duration::from_millis(300), LOCK_STALE_AFTER_5M)`, timed with `Instant`.
- Assert `Err(TaskOpError::LockIo(p, e))` with `p == lock`, `e.raw_os_error() == Some(5)`, elapsed
  `>= 300 ms`. The elapsed check proves the retry; the variant proves the deadline rule.
- Windows-only: on Unix `O_EXCL` on a directory is `EEXIST`, the unchanged `AlreadyExists` path.

**T3 `issue_1579_acquire_retries_access_denied_race`** (`#[cfg(windows)]`, the detector)
- Fixture `task-1579-race`, lock path `TASK.md.lock`.
- Four hammer threads until an `AtomicBool` stop: `if let Ok(f) = OpenOptions::new().write(true)
  .create_new(true).open(&p) { drop(f); let _ = std::fs::remove_file(&p); }`. A hammer removes only
  a file it created.
- Main thread for 8 s wall: `let g = LockGuard::acquire(&p, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M);`
  On `Err`, set stop, join hammers, then `panic!("acquire {n}: {e:?}")`. On `Ok`, drop and count.
- After the loop: stop, join, assert count `> 0`.
- Post-fix it can only fail by starving for 5 s (each attempt about 73% `AlreadyExists` in the
  probe; 100 losses in a row is about 2e-14). Pre-fix it fails in about 3 s locally (§1).

T1 alone is not proof: it pins the table, not the wiring.

## 5. Proof protocol (Grinch veto)

Run from `src-tauri`, on Windows, one test thread:

```
cargo test --lib issue_1579 -- --test-threads=1 --nocapture
```

1. **Positive control, pre-fix.** Add T1-T3 and the §2.1 classifier (T1 needs it to compile), but
   not the §2.2 arm. Expected: T1 green; T2 red (`LockIo` os 5 in well under 300 ms, elapsed
   assertion fails); T3 red with `LockIo(... Os { code: 5, kind: PermissionDenied ...})`. Run T3
   5 times; every run must be red. Capture raw output.
2. **Fix.** Add §2.2. Same command, 5 runs: all green.
3. **Mutations** (apply each alone, run, capture red, revert):
   - M1: classifier returns `false` on Windows. T1 and T3 red, T2 red.
   - M2: deadline returns `LockTimeout` unconditionally in the new arm. T2 red.
   - M3: deadline returns `LockIo` unconditionally in the new arm. Expect T1-T3 green (the race
     never hits the deadline); `lock_guard_blocks_concurrent_acquisition` also green because it is
     the `AlreadyExists` arm. M3 is a **surviving mutant**, recorded, not a blocker: killing it needs
     a sustained 5 plus a real holder, which cannot be produced deterministically.
   - M4: remove `saw_already_exists = true;`. Same survival and reason as M3.
4. Grinch reviews steps 1-3 raw outputs before acceptance.

## 6. Gates (owner: dev, before push; then CI)

From `src-tauri`:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib cli::task_ops -- --test-threads=1
cargo test --lib --bins --tests
```

All green; `concurrent_writes_return_correct_post_edit_content` passes unmodified. The full
Windows suite and clippy run in `.github/workflows/pr-regression-gates.yml` job `rust-regression`
(`windows-latest`, `:54`, `:90-96`). Delivery: every triggered and required check green on the
exact PR head SHA. A green run of the old concurrent test proves nothing about this fix (issue
body); only §5 does.

## 7. Git and scope

- Before the first edit: `git fetch origin main`; `git status --porcelain` empty;
  `git rev-parse HEAD` = base or a descendant with unrelated drift only. Drift touching
  `cli/task_ops.rs` or `pr-regression-gates.yml` means re-anchor §2/§4 and tell the coordinator.
- Final: `git diff --name-only 5203c4e3...HEAD` lists exactly `src-tauri/src/cli/task_ops.rs` and
  `plans/1579-lock-access-denied-retry.md`; no untracked leftovers.
- Recovery: revert only your own hunks in `task_ops.rs`; no `git reset --hard`, no repo-wide clean.
- One PR into `main`, `Closes #1579` only (never #1578).

## 8. Acceptance criteria

1. Windows 5/32 on the lock open retries under the same deadline and poll (§2.2, T3).
2. Persistent 5/32 with no holder seen returns `LockIo` (T2).
3. Real contention past the deadline still returns `LockTimeout` (existing
   `lock_guard_blocks_concurrent_acquisition`).
4. Stale check unchanged and not bypassed (existing `lock_guard_recovers_stale_lockfile`).
5. Non-Windows unchanged (T1 on Linux CI, if run; by construction otherwise).
6. §5 raw outputs delivered and reviewed by Grinch; §6 gates green; exact-head CI green.

## 9. Risks

| Risk | Assessment |
|---|---|
| T3 on CI (fewer cores) | Post-fix direction cannot flake except by 5 s starvation (negligible). Pre-fix detection rate on CI is unmeasured; the local §5 control is the proof |
| T3 adds about 8 s to the Windows suite | Accepted; the race needs wall time |
| Windows env (dev must state in writing) | Detection depends on OS timing and core count; this box has Defender real-time off, so AV-induced 32 cannot be reproduced; 32 retry is justified by policy, not a local repro |
| Directory/ACL failures now take 5 s | Error variant preserved; latency only |
| Cold Rust build in the replica | Slow first run, not a hang |

## Plan Contract

Scope: `src-tauri/src/cli/task_ops.rs` per §2 and §4, plus this plan. The dev runs §5 and §6 and
reports raw outputs, commit SHA, file list and the written Windows env risk. Any change to the
concurrent test, the timeouts, the stale check, non-Windows behavior, or any file beyond scope
stops work and returns to the coordinator.
