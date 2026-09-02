# Phase 1 (#1724): Rust gate, resolved flag, and the Tauri command

Status: READY_FOR_IMPLEMENTATION
Class: `patterned`
Owner: `ac-dev-rust-v4`
Depends on: nothing. Parallel with: nothing.
Branch: `feature/1724-testable-pulse-suppression-switch`, base `1e57aa581de4c4fd18590cdf0652d8bf60b18a4f`.
Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-AgentsCommander`.

## Objective

Resolve, once at process startup and behind the existing UI-automation gate, a boolean saying whether the sidebar layout pulse is suppressed; hold it immutably on `UiAutomationState`; and expose it to the webview through one new Tauri command. Nothing in this phase changes any behavior the user can observe: the command exists and returns `false` on every binary that is not `agentscommander_testeable.exe` running with UI automation and the opt-in variable set.

## Exact files (freeze this set; nothing else may change)

1. `src-tauri/src/testability/ui_automation.rs`
2. `src-tauri/src/commands/testability.rs`
3. `src-tauri/src/lib.rs`

## Decisions, all already made

The switch is on if and only if all three hold:

- UI automation is enabled for this process, which `resolve_enabled_from_cli_or_env` already decided from `--ui-automation` or `AC_UI_AUTOMATION=1`;
- the running executable's file name is exactly `TESTABLE_EXE_NAME`, that is `agentscommander_testeable.exe`;
- the environment variable `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE` is exactly the string `1`.

Any other combination is `false`. Resolution happens once, at `UiAutomationState` construction time in `run()`, and the value is immutable for the process. There is no runtime toggle and no CLI flag: a toggle could land mid-pulse and would need mid-flight cancellation semantics plus a new `UiAutomationAction` variant, and a clap flag would widen `Cli`, `main.rs` and the public `run()` signature for no gain over the environment variable, which is already the first-class peer of `--ui-automation`.

Refusal is silent (the resolver returns `false`), not an error that aborts startup. A stray environment variable must never stop a release binary from launching. The refusal is proven by unit tests on the pure resolver, not by a startup failure.

The on-disk `UiAutomationSession` record (`ui_automation.rs:139-147`) is **not** touched: it exists so a separate CLI process can address the running GUI, and suppression is never addressed by the CLI.

## Edit 1: `src-tauri/src/testability/ui_automation.rs`

### 1a. New constant, beside the existing `ENV_ENABLE`

`ENV_ENABLE` is at line 21. Add immediately after it:

```rust
pub const ENV_SUPPRESS_LAYOUT_PULSE: &str = "AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE";
```

### 1b. Factor the exe-name read out of `current_exe_is_testable`, without changing its behavior

Current text at lines 1624-1632:

```rust
fn current_exe_is_testable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|name| name == TESTABLE_EXE_NAME)
}
```

Replace with:

```rust
fn current_exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

fn current_exe_is_testable() -> bool {
    current_exe_name() == TESTABLE_EXE_NAME
}
```

This is behavior-preserving: the old `is_some_and` yielded `false` when the name could not be read, and the new form compares the empty string against a non-empty constant, which is also `false`. Both callers of `current_exe_is_testable` (`ensure_current_exe_is_testable` at line 1612 and `resolve_enabled_from_cli_or_env` at line 1475) are unchanged.

### 1c. The resolver, placed immediately after `resolve_enabled_from_cli_or_env` (which ends at line 1484)

```rust
/// Pure core of the layout-pulse suppression gate, mirroring
/// `window_placement::resolve_from_cli_or_env_for_exe`: every input is a parameter so
/// the gate can be proven in a unit test without touching process state. All three
/// conditions are required, and the environment value must be exactly "1".
fn resolve_layout_pulse_suppression_for_exe(
    automation_enabled: bool,
    exe_name: &str,
    env_value: Option<&str>,
) -> bool {
    automation_enabled && exe_name == TESTABLE_EXE_NAME && env_value == Some("1")
}

/// Resolved once at startup and then immutable for the process, so no in-flight
/// sidebar pulse can ever observe the value changing. Refusal is silent by design: a
/// stray variable on a release binary must not prevent the app from starting.
pub fn resolve_layout_pulse_suppression(automation_enabled: bool) -> bool {
    let env_value = std::env::var(ENV_SUPPRESS_LAYOUT_PULSE).ok();
    resolve_layout_pulse_suppression_for_exe(
        automation_enabled,
        &current_exe_name(),
        env_value.as_deref(),
    )
}
```

### 1d. The immutable field on `UiAutomationInner` and the widened constructor

`UiAutomationInner` is at lines 249-261. Add a field directly after `enabled: bool`:

```rust
    layout_pulse_suppressed: bool,
```

`UiAutomationState::new` is at lines 264-290. Change its signature to

```rust
    pub fn new(enabled: bool, layout_pulse_suppressed: bool, config_dir: PathBuf) -> Self {
```

and initialize the new field in the `UiAutomationInner { .. }` literal as `layout_pulse_suppressed,`. Every other field keeps its current initializer verbatim.

#### The seven existing call sites, every one of which moves to the new arity

A repo-wide grep for `UiAutomationState::new` at the pinned base returns exactly seven hits: one in production and six inside this same file's `#[cfg(test)] mod tests`. Widening the constructor breaks all seven with E0061 until each is updated, and the six test hits are `#[cfg(test)]` items of the lib crate, so leaving them behind reddens `cargo check --all-targets`, `cargo clippy --workspace --all-targets`, and every `cargo test` invocation in this phase.

| # | Location at `1e57aa5` | Enclosing item | Current text |
|---|---|---|---|
| 1 | `src-tauri/src/lib.rs:2427` | `run()`, production | see Edit 3a |
| 2 | `ui_automation.rs:2919` | `fn complete_rejects_unknown_request_id` | `let state = UiAutomationState::new(true, tmp.path().to_path_buf());` |
| 3 | `ui_automation.rs:2938` | `fn frontend_ready_registers_dynamic_caller_label` | identical line |
| 4 | `ui_automation.rs:2960` | `fn live_window_sync_prunes_closed_dynamic_windows` | identical line |
| 5 | `ui_automation.rs:2979` | `fn complete_writes_completion_mismatch_response` | identical line |
| 6 | `ui_automation.rs:3025` | `fn expire_pending_requests_writes_timeout_response` | identical line |
| 7 | `ui_automation.rs:3072` | `fn initialization_failure_can_disable_state` | identical line |

Call site 1 is Edit 3a below. **Decision, made here and not delegated: call sites 2 through 7 each pass `false` for the new parameter.** All six are byte-identical, eight-space-indented lines, so this is one mechanical rewrite applied six times:

```
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
```

becomes

```
        let state = UiAutomationState::new(true, false, tmp.path().to_path_buf());
```

`false` is the correct value, not merely the convenient one. Each of those six tests builds an *enabled* automation state in order to exercise the session file, the window-label sets, request completion, request expiry, or the initialization-failure path; not one of them reads, asserts on, or is reachable from the layout pulse. `false` is exactly the value each of them has today by absence, so behavior and intent are preserved, and `true` stays confined to the one new test in Edit 1f that exists to prove the accessor. Change no other byte of those six tests: same names, same fixtures, same assertions.

The rewritten line is 80 columns, inside `rustfmt`'s 100-column default (this repository has no `rustfmt.toml`), so the formatter leaves it on one line. Run `cargo fmt --all` anyway and let it confirm.

Keeping the arity fixed was considered and rejected. `UiAutomationInner` lives behind an `Arc` (`ui_automation.rs:246`), so setting the field after construction would need `Arc::get_mut` plus an `expect`, and a second constructor that defaults the field to `false` would reintroduce precisely the silent default this design exists to remove. One constructor in which the value must be decided is what makes the field's immutability mean anything; the price is six identical one-token edits in a file this phase already owns, inside the frozen three-file set.

### 1e. The accessor, placed directly after `enabled()` (lines 292-294)

```rust
    /// ANDs `enabled()` so suppression without an enabled automation session is not
    /// merely unreached but structurally unrepresentable, whatever the stored field says.
    pub fn layout_pulse_suppressed(&self) -> bool {
        self.enabled() && self.inner.layout_pulse_suppressed
    }
```

### 1f. Five unit tests, appended inside the existing `#[cfg(test)] mod tests` (opens at line 2062)

The module already has `use super::*;`. Add:

```rust
    #[test]
    fn layout_pulse_suppression_enabled_when_gate_and_env_hold() {
        assert!(resolve_layout_pulse_suppression_for_exe(
            true,
            TESTABLE_EXE_NAME,
            Some("1")
        ));
    }

    #[test]
    fn layout_pulse_suppression_refuses_non_testeable_exe() {
        for exe in [
            "agentscommander.exe",
            "agentscommander_stage.exe",
            "AGENTSCOMMANDER_TESTEABLE.EXE",
            "",
        ] {
            assert!(
                !resolve_layout_pulse_suppression_for_exe(true, exe, Some("1")),
                "exe {exe} must not be able to enable layout-pulse suppression"
            );
        }
    }

    #[test]
    fn layout_pulse_suppression_refuses_without_automation() {
        assert!(!resolve_layout_pulse_suppression_for_exe(
            false,
            TESTABLE_EXE_NAME,
            Some("1")
        ));
    }

    #[test]
    fn layout_pulse_suppression_defaults_off() {
        for env_value in [None, Some(""), Some("0"), Some("true"), Some("11"), Some(" 1")] {
            assert!(
                !resolve_layout_pulse_suppression_for_exe(true, TESTABLE_EXE_NAME, env_value),
                "env value {env_value:?} must not enable layout-pulse suppression"
            );
        }
    }

    #[test]
    fn layout_pulse_suppressed_accessor_requires_enabled_state() {
        let dir = std::path::PathBuf::from(r"C:\tmp\ac-1724-accessor");
        assert!(!UiAutomationState::new(false, true, dir.clone()).layout_pulse_suppressed());
        assert!(UiAutomationState::new(true, true, dir.clone()).layout_pulse_suppressed());
        assert!(!UiAutomationState::new(true, false, dir).layout_pulse_suppressed());
    }
```

`UiAutomationState::new` only builds paths and in-memory collections; it touches no filesystem (only `start()` does), so the fabricated directory is safe and nothing is created on disk.

Deliberately **not** tested: `resolve_layout_pulse_suppression` itself. Inside a cargo test binary `current_exe()` is `target/debug/deps/<name>.exe`, so that wrapper would always return `false` regardless of the environment, which would be a green test proving nothing. The gate is proven on the pure core, and the wrapper is three lines of parameter passing.

## Edit 2: `src-tauri/src/commands/testability.rs`

Add directly after `ui_automation_enabled` (lines 3-8), keeping the same shape:

```rust
#[tauri::command]
pub fn ui_automation_layout_pulse_suppressed(
    state: State<'_, crate::testability::ui_automation::UiAutomationState>,
) -> bool {
    state.layout_pulse_suppressed()
}
```

No new `use` is required: `State` is already imported at line 1 and the state type is already referenced by path in this file.

## Edit 3: `src-tauri/src/lib.rs`

### 3a. The construction site, lines 2427-2430

Current:

```rust
    let ui_automation_state = crate::testability::ui_automation::UiAutomationState::new(
        ui_automation_enabled,
        config_dir.clone(),
    );
```

Replace with:

```rust
    let ui_automation_state = crate::testability::ui_automation::UiAutomationState::new(
        ui_automation_enabled,
        crate::testability::ui_automation::resolve_layout_pulse_suppression(ui_automation_enabled),
        config_dir.clone(),
    );
```

`run()`'s signature is unchanged; no new parameter crosses the `main.rs` boundary.

### 3b. The registration, line 3573

Add one line directly after `commands::testability::ui_automation_enabled,`:

```rust
                commands::testability::ui_automation_layout_pulse_suppressed,
```

## Required behavior, edge cases and failure behavior

- Missing variable, empty value, `"0"`, `"true"`, `"11"`, `" 1"`, or any value other than the exact string `"1"`: suppression is off. Comparison is exact and case-sensitive on both the value and the executable name.
- Executable name is compared case-sensitively against the literal `agentscommander_testeable.exe`, exactly as the existing bridge gate does. A binary named anything else cannot enable the switch even with the variable set and even with `--ui-automation` passed, because that combination already fails at `resolve_enabled_from_cli_or_env`.
- `current_exe()` failing: `current_exe_name()` yields the empty string, which does not equal the constant, so suppression is off and `current_exe_is_testable()` keeps returning `false` exactly as before.
- Automation disabled, or the session later marked unavailable: `enabled()` is `false`, so the accessor returns `false` even if the stored field is `true`.
- The command is callable from any webview window. It reads process-lifetime state, takes no argument, cannot fail, and returns `false` in every non-qualifying case, so there is no error path and no `Result`.

## Formatting

Run `cargo fmt --all` from `src-tauri` before committing and let it decide every line break. The Rust above is written for semantics, not layout; do not hand-format it and do not fight the formatter. `rust-fmt` is a required CI check (`cargo fmt --all -- --check`), and it reformats whole items, so an unformatted addition fails the gate even though it compiles.

Two clippy traps in this file's neighbourhood: `-D warnings` is in force, so a doc comment whose continuation line starts with a bare ordinal (`1.`, `2.`) trips `doc_lazy_continuation` and becomes an error. The doc comments given above avoid it; keep it that way if you rewrite them.

## Verification commands, from `src-tauri`

```
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib layout_pulse_suppress
cargo test --lib --bins --tests
```

Redirect `cargo test` output to a file and read it back; piping it from some shells swallows stdout. Read the **last** `test result:` line, not the first.

Expected results:

- `cargo fmt --all -- --check`: exit 0, no diff.
- `cargo check`, `cargo clippy`: exit 0, zero warnings.
- `cargo test --lib layout_pulse_suppress`: exactly 5 tests, all passing. If the count is not 5, a test was renamed or never compiled; that is a failure, not a filter to loosen.
- `cargo test --lib --bins --tests`: no new failure relative to the base SHA. This repository has known pre-existing flakes in the Rust regression suite; classify any failure against a base-SHA run before attributing it to this phase, and report the comparison rather than a bare pass or fail.

## Acceptance criteria

1. `git status --porcelain` lists exactly the three files above, and nothing else. In particular `src-tauri/module-arcs.txt` is unchanged.
2. All five verification commands meet their expected results, with captured output and exit codes.
3. `cargo test --lib layout_pulse_suppress` reports 5 passed, 0 failed.
4. `grep -c "ui_automation_layout_pulse_suppressed" src-tauri/src/lib.rs` is 1 and `grep -c "ui_automation_layout_pulse_suppressed" src-tauri/src/commands/testability.rs` is 1.
5. Every one of the seven call sites of Edit 1d moved to the new arity. From the repository root:

   ```
   grep -c "UiAutomationState::new(true, false, tmp.path().to_path_buf())" src-tauri/src/testability/ui_automation.rs
   grep -c "UiAutomationState::new(true, tmp" src-tauri/src/testability/ui_automation.rs
   grep -rn "UiAutomationState::new" src-tauri/ --include=*.rs
   ```

   Expected: 6, then 0, then exactly ten matching lines: `lib.rs:2427`, the six rewritten test lines, and the three separate `assert!` lines added by Edit 1f. `grep -c` counts matching lines and prints 0 with exit 1 when there is no match, so read the printed number, not the exit code. Call site 1 in `lib.rs` is additionally proven by `cargo check --all-targets`, which cannot pass while any of the seven still has the old arity (E0061).
6. No new module arc. From the repository root:

   ```
   node "D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
   npm run record:arcs -- --graph graph.json
   git status --porcelain src-tauri/module-arcs.txt
   ```

   Expected: the detector exits 1 and writes the graph anyway, which is the normal outcome on this repository because one pre-existing gating SCC already exists; only exit 3 means no graph was written. `record:arcs` defaults its `--out` to `src-tauri/module-arcs.txt`. `git status --porcelain src-tauri/module-arcs.txt` then prints nothing, proving byte-identity. Delete `graph.json` afterwards; it sits at the repository root, is gitignored (`.gitignore:34`) and must never be committed because it carries this machine's absolute paths. A non-empty status on `src-tauri/module-arcs.txt` is a failure of this phase, not a regeneration to commit. The three arcs this phase exercises are already recorded at `module-arcs.txt:43`, `:532` and `:1038`; the fourth reference, the `generate_handler!` entry of Edit 3b, is invisible to the detector and adds no record line. The pre-versus-post SCC comparison over the finished branch stays an epic-level gate (`epic.md` section 7); this criterion is the per-phase byte-identity check and is executable without that file.
7. Record the resolved `cargo --version`, `rustc --version` and `node --version` in the phase report.

## Preserve list (must not change in this phase)

- `run()`'s signature in `src-tauri/src/lib.rs` and every caller of it in `src-tauri/src/main.rs`.
- `src-tauri/src/cli/mod.rs`: no new flag, no new subcommand.
- `resolve_enabled_from_cli_or_env`, `ensure_current_exe_is_testable`, `automation_not_enabled_error`, and every existing refusal string and error code.
- `UiAutomationAction` and its `next_variant` / `all` walk, the `UiAutomationSession` struct and the session file format, `UiAutomationRequest`, `UiAutomationResponse`.
- The six existing tests listed in Edit 1d, apart from the single added `false` argument: their names, fixtures, assertions and expected values are unchanged, and none of them gains a layout-pulse assertion.
- Any file under `src/`, any file under `docs/`, `package.json`, `package-lock.json`, `Cargo.lock`, `src-tauri/Cargo.toml`, and every workflow in `.github/workflows/`.
