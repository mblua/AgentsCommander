use super::*;

#[cfg(windows)]
mod blocked_writer_teardown;
mod cancel;
#[cfg(windows)]
mod conpty;
mod context_gate;
mod context_liveness;
mod probe_containment;
mod resize_dedup;
mod startup_gate;
// Pure adapter and closed configured-host grammar tests. Platform
// independent: they exercise the classifier, encoders, and generators directly,
// not through the real PTY path. The spawn_sync ordering proof lives in
// `adapter_spawn_sync_tests` (windows-gated, real backend).
mod adapter;
// spawn_sync ordering proof: deterministic invalid-input rejections
// happen at the TOP of the REAL `spawn_sync` (before `note_spawn_attempt` and
// before `native_pty_system()`), so the per-instance observer stays at zero,
// no PTY map entry exists, and no launch provenance was recorded. Each test
// builds its own backend instance and reads its own observer; there is no
// global to reset and no cross-test race.
#[cfg(windows)]
mod adapter_spawn_sync;
/// T5: the search path is applied before the env-removal and
/// configured-env loops in `spawn_sync`, so explicit configuration still wins.
/// T6: `GitWatcher::new` (test-only; production uses `with_state` with the
/// same `E: GitEvents` bound) accepts the native `AppHandle` as its `GitEvents`
/// implementation.
mod issue_2683_search_path_order;
/// T17: the local backend marks the launch witness only once a child
/// exists, so a `spawn_command` failure stays restorable.
mod launch_witness;
/// T1/T2: the REAL `spawn_sync` resolves a bare program name through the
/// search path its production adapter call applies. The value is injected with
/// the thread-local `SearchPathOverrideGuard`, which is only visible because
/// `spawn_sync` runs the whole builder on the calling test thread. Headless:
/// no `tauri::App` is built, so no GTK is needed on a display-less runner.
#[cfg(unix)]
mod spawn_sync_search_path;
/// P02: Git brakes are skipped only for an unleashed replica.
mod unleash_git_brakes;
