# #1592 — fatal startup messages survive a windowless launch

Status: READY_FOR_IMPLEMENTATION

Issue: `#1592`  
Repository: `repo-AgentsCommander`  
Target: `main` pinned at `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e`  
Branch: `fix/1592-startup-message-surface`  
Delivery: Full

## Objective and evidence

A Windows release binary uses `windows_subsystem = "windows"` (`src-tauri/src/main.rs:1`). When a same-identity GUI instance already owns the mutex, the plain GUI branch exits silently with code 0 (`main.rs:113-130`). Merely printing before that exit is insufficient after Explorer launch: `attach_parent_console()` may allocate a process-owned console, which disappears at process exit.

Instance identity is executable-name based, not directory based. `binary_suffix()` splits the executable stem at the first underscore (`src-tauri/src/config/profile.rs:20-30`); `mutex_name()` and `web_server_port()` consume that suffix (`profile.rs:83-96`, `159-175`). Thus two plain `agentscommander.exe` copies collide, while `agentscommander_<name>.exe` owns independent mutex/configuration/ports.

The compatibility constraint is load-bearing. `attach_parent_console()` classifies each standard handle as invalid only when it is null or `GetFileType(handle) == FILE_TYPE_UNKNOWN`, and attaches/allocates only when **both** are invalid (`src-tauri/src/cli/mod.rs:232-269`). Its comment records issue #129: unconditional `AttachConsole` rebinds valid inherited pipe/file/console handles and drops captured output. No console-presence test, including `GetConsoleWindow()`, may replace or supplement this predicate.

Task class: routine application/test change with a concrete Windows UI and CI-hang regression hazard. Accepted threat model: trusted developer and GitHub-hosted build machines, trusted repository toolchain/lockfiles, and local OS APIs. No signing, supply-chain provenance, untrusted-host, destructive migration, secret, or security-boundary change exists; enhanced provenance/concurrency controls are not applicable.

## Scope

Change only:

- `src-tauri/src/cli/mod.rs`
- `src-tauri/src/main.rs`
- `scripts/smoke-cli-powershell.ps1`

Do not change Cargo/package dependencies or lockfiles; `windows-sys 0.59` already enables `Win32_UI_WindowsAndMessaging`. Do not change `src-tauri/module-arcs.txt`; it is a validation oracle and must remain byte-identical.

Out of scope: #1577's resolver and `lib.rs:1869` call-site replacement; #1591's WebView2 preflight; other existing startup-print call sites; focusing an existing window; `binary_suffix()`; non-fatal output; release-track documentation; README, quickstart, portable-instance, or privacy files.

## Decided solution

### Shared contract and exact API

Add this public, process-agnostic API beside the existing console/output helpers in `src-tauri/src/cli/mod.rs`:

```rust
pub fn present_fatal_startup_message(message: &str)
```

It returns `()` and never exits. It owns presentation and stream flushing; the caller owns termination and therefore preserves its path-specific exit code. It must not require Tauri, an `AppHandle`, a window, logger initialization, or UI-automation state.

Add these private implementation seams in the same module, with these names and roles:

- `StartupMessageDestination::{NativeDialog, Stderr, Stdout}`.
- `startup_message_destination(stdout_invalid: bool, stderr_invalid: bool) -> StartupMessageDestination`, compiled for Windows and unit tests. This is the single decision function: `(true, true)` is `NativeDialog`; otherwise prefer `Stderr` when stderr is valid, and use `Stdout` only when stderr is invalid and stdout is valid.
- Windows-only `standard_handle_is_invalid(handle)` retaining the exact existing null-or-`FILE_TYPE_UNKNOWN` rule.
- Windows-only `standard_handle_state()` reading stdout and stderr once and returning the two invalidity booleans.
- Windows-only `show_native_startup_message(message: &str)`, containing only UTF-16/NUL conversion and the blocking native call.

Refactor `attach_parent_console()` to consume `standard_handle_state()` and attach/allocate only when `startup_message_destination(...) == NativeDialog`. This mechanically shares the existing predicate; its behavior and all existing callers remain unchanged.

### Windows presentation

`present_fatal_startup_message` reads the same handle state once and dispatches as follows:

| stdout | stderr | Destination and behavior |
| --- | --- | --- |
| invalid | invalid | Call `MessageBoxW(NULL, message, "AgentsCommander startup blocked", MB_OK | MB_ICONERROR | MB_SETFOREGROUND)` and block until the user closes or acknowledges it. Do not attach or allocate a console. |
| any | valid | `eprintln!("{message}")`, then `flush_outputs()`. |
| valid | invalid | `println!("{message}")`, then `flush_outputs()`. |

Invalid retains its current meaning: null or `FILE_TYPE_UNKNOWN`. `FILE_TYPE_CHAR`, `FILE_TYPE_PIPE`, and `FILE_TYPE_DISK` are valid. `MessageBoxW` has no owner because no Tauri window exists. Do not add `MB_TOPMOST`, `MB_SYSTEMMODAL`, `MB_SERVICE_NOTIFICATION`, a polling loop, or a second blocking surface. Closing the box is dismissal. A zero return from `MessageBoxW` is an OS presentation failure: return to the caller without changing its exit code; do not hang, retry indefinitely, allocate a flashing console, or panic.

Messages are trusted in-process diagnostics. The helper adds no prefix and accepts no external formatting contract. Stream destinations add the one newline already expected from `eprintln!`/`println!`; the dialog displays the supplied content. Embedded NUL is unsupported and no planned consumer supplies it.

### Non-Windows and automation

On Linux and macOS, `present_fatal_startup_message` writes the supplied message to stderr, flushes, and returns. It never blocks or introduces a platform GUI dependency; `windows_subsystem` is inapplicable and supported launches normally have a terminal or redirection.

Do not add automation suppression to the helper. In the current second-instance path, the existing `ui_automation_enabled` branch completes before the new call: an existing enabled session still exits 0, and the automation-not-enabled JSON path still prints and exits 1. Automated fatal-path probes provide valid redirected handles, so they select a nonblocking stream destination. A global suppression would hide future #1577/#1591 diagnostics and add the wrong dependency.

### Second-instance consumer

Replace only the plain GUI branch's silent exit with:

```text
An AgentsCommander instance with this executable identity is already running.

Rename this executable to agentscommander_<name>.exe to start an independent instance with its own configuration directory and ports.
```

Pass that exact text (without a trailing newline in the string literal) to `present_fatal_startup_message`, then retain `std::process::exit(0)`. Exit 0 is compatibility behavior for an already-satisfied single-instance launch and is asserted by the release smoke. Do not move or edit either UI-automation branch or its exit codes.

Future #1577 may call `crate::cli::present_fatal_startup_message(&message)` when it replaces the `lib.rs:1869` `expect`; future #1591 may call it after a failed `tauri::webview_version()` preflight. Each future issue decides and performs its own nonzero termination. This delivery adds neither call.

## Required launch behavior and failure containment

| Launch shape | Required result | Evidence owner |
| --- | --- | --- |
| Explorer; both handles invalid/unknown | One native dialog with the exact message remains until dismissal; no allocated-console flash; process exits 0 after dismissal. | WG6 release owner, manual packaged-release check. |
| Real PowerShell/cmd terminal; `FILE_TYPE_CHAR` | No dialog or handle rebind; exact message goes to inherited stderr; exit 0. | WG6 release owner, manual release check. |
| PowerShell/ProcessStartInfo capture; `FILE_TYPE_PIPE` | No dialog or attach/rebind; exact message reaches captured stderr; exit 0 within the bound. Existing #129 CLI captures remain green. | Unit test plus Windows release smoke/CI. |
| Both streams redirected to files with no console; `FILE_TYPE_DISK` | No dialog; exact message reaches the stderr file; process exits 0 within the bound. | Windows release smoke/CI. |
| Only stdout valid | No dialog; exact message uses stdout and flushes. | Destination truth-table unit test. |
| Only stderr valid | No dialog; exact message uses stderr and flushes. | Destination truth-table unit test. |

The redirected-file binary criterion is required. It is the smallest permanent detector for the decision-2 constraint and for the #1593 `Start-Process -RedirectStandardOutput/-RedirectStandardError -Wait` hazard: a wrong console-presence predicate becomes a bounded, diagnostic test failure instead of an unexplained workflow timeout.

## Implementation order

1. Recheck the authorized root, branch, pinned base, clean index/worktree, and the three frozen paths. Fetch live `origin/main` and classify drift from the pinned base by those paths, Cargo/Node/toolchain configuration, `.github/workflows/pr-regression-gates.yml`, `.github/workflows/bundle-validation.yml`, and configured required checks. Refresh only evidence affected by relevant drift; unrelated target movement does not reopen this design.
2. In `cli/mod.rs`, extract the handle-state and pure destination seams, refactor `attach_parent_console()` onto them without changing its predicate, then add the Windows/non-Windows `present_fatal_startup_message` implementations and thin `MessageBoxW` wrapper.
3. Add inline unit tests: all four invalidity truth-table combinations; null and unknown handles classify invalid; on Windows, a real `Stdio::piped()` child handle and a real `tempfile` file handle classify valid. Tests must not replace process std handles or invoke `MessageBoxW`.
4. In `main.rs`, change only the plain second-instance branch to present the exact message and retain exit 0. Leave the preceding UI-automation branch byte-for-byte unchanged.
5. Extend `scripts/smoke-cli-powershell.ps1` only for `agentscommander.exe`. Give each spawned test process a child-scoped `AC_UI_AUTOMATION=0` and remove child-scoped `AC_TEST_WINDOW_PLACEMENT`; do not mutate the parent environment. Hold `Local\AgentsCommander_SingleInstance` open with `System.Threading.Mutex`, require that this test created the mutex, and in `finally` dispose it. Add:
   - a direct `ProcessStartInfo` case with both streams piped;
   - a child-shell `Start-Process -NoNewWindow -RedirectStandardOutput <file> -RedirectStandardError <file> -PassThru -Wait` case so the binary receives `FILE_TYPE_DISK` handles and no console.

   Each case has a 15-second `WaitForExit` bound. On timeout, terminate only that process tree with `taskkill /PID <pid> /T /F`, retain command/stdout/stderr logs in the existing smoke artifact directory, and fail. Assert exact normalized stderr text, byte-empty stdout, and exit 0. Do not run the modal branch in automation.
6. Run the local checks and structural comparison below, inspect the final diff/scope, then deliver through an issue-linked PR. The architect does not implement, commit, push, or publish this plan.

## Validation and objective acceptance

Use explicit repository/source working directories, repository lockfiles, Node 22 and npm 11.6.2 to match CI, and Rust stable as defined by the workflows. Commands that can compile or spawn processes require a runner timeout (30 minutes for build/check/test, 20 minutes for the release smoke), closed interactive stdin, captured exit/timing/stdout/stderr, and preserved smoke logs.

Local implementer checks, from the repository root unless a working directory is shown:

```text
node --version
npm --version
rustc -Vv
cargo --version
npm ci
npm run build
cargo fmt --all -- --check
cd src-tauri
cargo check --locked --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --lib --bins --tests
cargo test --locked --test loops_layering --test instance_gitignore_layering --test project_settings_layering
cd ..
npm run build:prod:no-bundle
npm run smoke:cli-release-windows
git diff --check
git status --short
```

Green means all commands exit 0; the new smoke records both startup cases as passed; no modal appears in automation; no unexpected tracked, staged, ordinary-untracked, lockfile, manifest, workflow, or configuration change exists. A timeout, skipped required case, missing log, changed UI-automation branch, unexpected generated file, or unexplained pre-existing failure is not accepted.

Manual acceptance, owned by the WG6 release owner after the implementer supplies the exact release/portable artifact:

1. Keep one plain `agentscommander.exe` identity running, double-click a second plain copy from another directory, verify the exact dialog remains until OK/close, then verify the second process exits 0.
2. Repeat the conflict from a real PowerShell or cmd terminal and verify exact stderr text, no dialog, and exit 0.
3. Rename the second copy to `agentscommander_<name>.exe` and verify it starts independently with its own configuration directory and ports.

Manual evidence must name the tested artifact SHA-256, Windows version, launch method, observed message/exit, tester, and timestamp. Failure blocks delivery; it does not authorize changing identity rules or exit codes.

## Dependency-cycle and layering gate

The clean pinned-base run produced 191 modules, 1,037 unique module arcs, 107 SCCs, and `cyclicSccs = 1`; the sole cyclic SCC has 85 members. Regenerating `src-tauri/module-arcs.txt` from that graph was byte-identical to the tracked 82,149-byte record (SHA-256 `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`). Detector exit 1 was the normal existing-cycle result; exit 3 would have invalidated the graph.

Planned new internal Rust module arcs: **zero**. Planned removed arcs: **zero**. `main.rs` already references `agentscommander_lib::cli`; the new `windows-sys::Win32::UI::WindowsAndMessaging` reference is external and its Cargo feature already exists. Inline tests add no production arc. No lower layer gains Tauri, `AppHandle`, UI transport, persistence, or testability ownership; the console-owning `cli` module remains the correct layer.

Before acceptance, create one absolute ignored validation directory and run the detector from clean checkouts/worktrees of the pinned base and final branch head. In each checkout, keep `src-tauri` as the target but write `pre.json` and `post.json` respectively into that same validation directory; then run levelization and the arc-record projection from the final repository root:

```text
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "<VALIDATION_DIR>\pre.json" --quiet
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "<VALIDATION_DIR>\post.json" --quiet
node "<VAULT>\rust\Levelization\02-levelize.mjs" rank "<VALIDATION_DIR>\pre.json" > "<VALIDATION_DIR>\pre-rank.json"
node "<VAULT>\rust\Levelization\02-levelize.mjs" rank "<VALIDATION_DIR>\post.json" > "<VALIDATION_DIR>\post-rank.json"
node scripts/02-module-arc-record.mjs --graph "<VALIDATION_DIR>\post.json" --out "<VALIDATION_DIR>\post-module-arcs.txt"
```

Accept only if detector exits are 0 or 1 and both graphs exist; `cyclicSccs` is 1 before and after; the one 85-member cyclic SCC is identical set-for-set and module-for-module; the unique `from -> to` arc sets are identical, hence zero cross-boundary additions; `post-module-arcs.txt` is byte-identical to tracked `src-tauri/module-arcs.txt`; that tracked file is clean; and the three structural guards above pass. Exit 3, a changed SCC member set, any new arc, or arc-record drift blocks delivery.

## Git, recovery, and CI delivery

Immediately before product writes, the implementer records the branch/base/index/status plus preimage hashes for the three scoped files and stores recoverable preimages under `target/1592-recovery/<UTC-run-id>/`. Record post-write hashes after each owned edit. On failure, restore only a path whose current hash still equals this run's recorded post-write hash; preserve and report any externally changed bytes. Never use broad reset/restore or repository-wide cleanup. After success or recovery, prove the intended path set, empty index unless intentionally staging, tracked diff, lockfiles, and ordinary untracked state.

At pinned base, `.github/workflows/pr-regression-gates.yml` runs `test-debt`, Windows/Linux/macOS Rust regression, terminal-snapshot portability, `windows-release-cli-smoke`, and frontend regression. The Windows job runs `cargo check`, clippy, and Rust tests; the release-smoke job builds the release binary and invokes the smoke modified here. `lockfile-drift` also reports on pull requests but must observe no package/lock change. Current `bundle-validation` and `version-sync` path filters do not match this three-file diff. PR #1593 is known relevant drift: if it lands before mutation or PR delivery and makes `bundle-validation` or portable smoke applicable, re-derive and require that job rather than preserving this base-era skip.

The branch is issue-numbered and #1592 is open. Deliver by PR, never direct push to `main`. GitHub branch protection is strict and currently requires `validate-branch-name`; that check and every other triggered/configured-required check must succeed for the **exact PR-head SHA** after any synchronization. A result from another SHA, unexplained skip, bypass, dirty scope, relevant unreviewed drift, or failed manual release evidence blocks delivery.

## Documentation, compatibility, and security

No persisted format, IPC/event contract, port/mutex algorithm, config ownership, dependency, lockfile, or network behavior changes. Valid stream behavior is preserved and #129 is explicitly guarded. The only user-visible changes are the requested second-instance message and a native blocking dialog when both standard handles are invalid. The message contains no secrets and is shown only to the local interactive user. Release-track documentation can now truthfully state that a same-name portable collision explains the rename workaround; documentation edits remain owned by that track.
