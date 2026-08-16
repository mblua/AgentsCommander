# Plan #1154-fix: make `rust-regression-linux` and `rust-regression-macos` pass on PR #1156

Author: architect, wg-8. Authored and certified in a single Lite pass on 2026-08-15 UTC.

Status: READY_FOR_IMPLEMENTATION

Certification note: `Plan-SHA256` is the SHA-256 of this file's exact bytes as written. No hash line is embedded in the file, so the value is reproducible with `Get-FileHash -Algorithm SHA256` on the committed path.

Issue: [mblua/AgentsCommander#1154](https://github.com/mblua/AgentsCommander/issues/1154).
Pull request: [#1156](https://github.com/mblua/AgentsCommander/pull/1156), base `main`, head `1deb4e77d9fe3786af54878cb18e168cf0416931`.
Branch: `ci/1154-nonwindows-rust-gates`.
Predecessor plan: `plans/1154-nonwindows-rust-gates.md` on the same branch.

---

## 0. Read this first: two findings that change the shape of the work

Both were verified, not inferred. They are stated up front because they invalidate part of the premise the plan request was written against.

### 0.1 The branch is 342 commits stale and the PR no longer merges

Measured on 2026-08-15 after `git fetch origin ci/1154-nonwindows-rust-gates:ci/1154-nonwindows-rust-gates`:

| Fact | Value |
|---|---|
| `git rev-list --count ci/1154-nonwindows-rust-gates..main` | `342` |
| `git rev-list --count main..ci/1154-nonwindows-rust-gates` | `1` |
| `git merge-base main ci/1154-nonwindows-rust-gates` | `1e7f2350b481918c1e63abdf86149630d924ef2f` |
| `origin/main` == local `main` | `c13a079f19c06f8a9f2c07149f2661da7cd9342f` |
| `gh api .../pulls/1156` -> `mergeable` | `false` |
| `gh api .../pulls/1156` -> `mergeable_state` | `dirty` |

The CI evidence in the plan request comes from workflow runs created `2026-07-26T19:18:22Z` against head `1deb4e77`, whose base was `1e7f2350`. GitHub can no longer compute a merge ref for this PR, so no run since then reflects current `main`.

Consequence: the defect inventory in the plan request is a snapshot of a tree that is three weeks and 342 commits behind the tree that will actually be merged. It is partly obsolete and partly incomplete. Section 2 restates it against current `main`.

### 0.2 Four of the ten reported Linux failures are already fixed on `main`

`git diff ci/1154-nonwindows-rust-gates..main -- src-tauri/src/commands/wg_delete_diagnostic.rs` is a four-line insertion that adds exactly the gating this plan would otherwise have had to design:

```
+#[cfg(windows)]
 const MAX_FILES_PER_PROCESS: usize = 5;
+#[cfg(any(windows, test))]
 fn win32_meaning(code: u32) -> &'static str {
+#[cfg(any(windows, test))]
 fn win32_error(context: &str, code: u32) -> DiagnosticError {
+#[cfg(windows)]
 fn diagnostic_error(message: impl Into<String>) -> DiagnosticError {
```

That is issue #1136's sibling #1131, `B08-WG-DIAGNOSTIC-CFG`, which is **closed**. Merging `main` into the branch resolves those four reported failures with zero new code.

---

## 1. Scope conflict that the tech lead and coordinator must acknowledge

This is flagged, not used as a reason to stop. The plan below is complete and implementable as requested.

Epic **#1113** decomposes the non-Windows Rust baseline into 14 numbered children, each of which the epic requires to land as a "separately reviewed maintenance outcome" with "its own focused PASS/FAIL seam". The residue this plan must fix maps one-to-one onto five of them, all currently **open**:

| Residue | Child issue | State |
|---|---|---|
| `RelDecodeError` Windows-only variants | #1132 `B09-PROJECT-CODEC-CFG` | open |
| `beep_serialize` field | #1133 `B10-WATCHDOG-BEEP-CFG` | open |
| `window_info` payload types | #1134 `B11-WINDOW-INFO-CFG` | open |
| `delete_workgroup_dir_backend` | #1135 `B12-WORKGROUP-TEST-CFG` | open |
| non-Windows `PlatformProcessTreeBackend` `Default` | #1136 `B13-PROCESS-TREE-DEFAULT` | open |

Two further defects have **no** child issue and are new work:

- the `SessionSelection::*_for_test` constructors (section 3.6), and
- the macOS `any_thread` compile break (section 3.7), which the epic never anticipated because its own completion gate runs `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` on the host, that is, on Windows.

Additionally, the predecessor plan `plans/1154-nonwindows-rust-gates.md` makes it a **binding non-goal** to "change anything under `src-tauri/` or `crates/`, including the `dead_code` and clippy defects that the new Linux job is expected to report", naming #1113 and #1131 through #1136 as the owners, and states that "a red clippy conclusion on the Linux job ... is the expected outcome of this change rather than a defect in it".

**Assumption under which this plan is certified:** the tech lead's written instruction ("Make them pass, on the same branch") is a deliberate reversal of that non-goal, taken with authority. Proceeding on that basis.

**Consequences the coordinator owns, not the implementer:**

1. PR #1156 will close #1132, #1133, #1134, #1135 and #1136 as a side effect. Their bodies and the epic checklist must be updated to say so, or they will be re-implemented later against code that already has the fix.
2. Two new children of #1113 (call them `B14-SELECTION-TEST-CFG` and `B15-TAURI-ANY-THREAD-MACOS`) should be filed to cover section 3.6 and section 3.7, even if their implementation lands here.
3. `validate-branch-name` passes on `ci/1154-nonwindows-rust-gates` regardless, so nothing is mechanically blocked.

Sections 3.1 through 3.7 are written as self-contained, independently applicable units so that if the coordinator instead rules that the fixes must go to per-issue branches, each unit lifts out with no rework.

---

## 2. Verified current state on `main` (`c13a079f`)

Every fact below was read directly at this base. Codebase Memory gate: `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-8-dev-v5-team-repo-AgentsCommander`, 23899 nodes, 130684 edges, indexed head `c13a079f`.

### 2.1 Toolchain and gate commands

- `tauri` resolves to `2.10.3` (`Cargo.lock:6011-6012`).
- Workspace root is the repository root (`Cargo.toml`, members `src-tauri`, `crates/session-bridge`, `crates/terminal-snapshot-renderer`). The lib crate is `agentscommander_lib` (`src-tauri/Cargo.toml` `[lib] name`), the package is `agentscommander-new`.
- Both new jobs run, with `working-directory: src-tauri`, `cargo check --all-targets` then `cargo clippy --all-targets -- -D warnings`. Without `--workspace`, cargo selects only `agentscommander-new`. This matches the reported error text `could not compile agentscommander-new`.
- `--all-targets` compiles `lib`, `lib test`, `bins` and every integration test crate under `src-tauri/tests/`. All four target kinds matter below.

### 2.2 The macOS blocker

`tauri-2.10.3/src/app.rs:1498-1506`:

```rust
/// - **macOS:** on macOS the application *must* be executed on the main thread, so this function is not exposed.
#[cfg(any(windows, target_os = "linux"))]
#[cfg_attr(docsrs, doc(cfg(any(windows, target_os = "linux"))))]
#[must_use]
pub fn any_thread(mut self) -> Self {
```

So the exact predicate under which `any_thread` exists is `any(windows, target_os = "linux")`. This is a hard `E0599`, not a lint, and it is why macOS fails at `cargo check` while Linux gets as far as `cargo clippy`.

There are **19** call sites on `main`, in 12 files:

| File | Lines | Target kind | Action |
|---|---|---|---|
| `src-tauri/src/commands/ac_discovery.rs` | 3170 | lib test | fix |
| `src-tauri/src/commands/config.rs` | 2694 | lib test | fix |
| `src-tauri/src/commands/session.rs` | 10489 | lib test | fix |
| `src-tauri/src/lib.rs` | 3601 | lib test | fix |
| `src-tauri/src/loops/delivery.rs` | 580 | lib test | fix |
| `src-tauri/src/pty/local_backend.rs` | 3852 | lib test | fix |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | 509 | lib test | fix |
| `src-tauri/src/web/commands.rs` | 1156, 1175, 1249, 1461, 1485 | lib test | fix |
| `src-tauri/src/web/event_broadcast.rs` | 41 | lib test | fix |
| `src-tauri/tests/pty_powershell_managed_native.rs` | 232, 258 | integration | fix |
| `src-tauri/tests/wake_consumption_measure.rs` | 209, 233 | integration | fix |
| `src-tauri/tests/pty_lifecycle_regression.rs` | 285, 313 | integration | **leave unchanged** |

`pty_lifecycle_regression.rs` line 1 is `#![cfg(target_os = "windows")]`, so that whole crate compiles to nothing on Linux and macOS. Its two sites are inert. Touching them would add diff for zero gate benefit, so they stay. The other two integration files carry no crate-level platform gate (`pty_powershell_managed_native.rs` line 1 is a doc comment, `wake_consumption_measure.rs` line 1 is a doc comment) and each contains exactly one `cfg(windows)` occurrence elsewhere, so their `any_thread` sites are live on macOS.

All 19 sites have the identical shape `tauri::Builder::default()` immediately followed by `.any_thread()`.

`acceptance_tests.rs` is reached through `#[cfg(test)] mod acceptance_tests;` at `src-tauri/src/pty/terminal_snapshot.rs:3147`, so it is a `lib test` site.

### 2.3 The Linux dead-code residue

Reported by the plan request, re-verified item by item against `main`:

| # | Reported symbol | Status on `main` | Owner issue |
|---|---|---|---|
| R1 | `MAX_FILES_PER_PROCESS` | **already fixed**, `#[cfg(windows)]` at `wg_delete_diagnostic.rs:105` | #1131 closed |
| R2 | `win32_meaning`, `win32_error` | **already fixed**, `#[cfg(any(windows, test))]` at `wg_delete_diagnostic.rs:108,120` | #1131 closed |
| R3 | `diagnostic_error` | **already fixed**, `#[cfg(windows)]` | #1131 closed |
| R4 | `WindowInfoOutput`, `WindowSnapshot`, `WindowRect` | **still broken** | #1134 |
| R5 | `RelDecodeError::{IllegalWindowsChar, ReservedDosName, TrailingDotOrSpace}` | **still broken** | #1132 |
| R6 | `beep_serialize` | **still broken** | #1133 |
| R7 | `delete_workgroup_dir_backend` | **still broken** | #1135 |
| R8 | `live_for_test`, `dormant_for_test`, `none_for_test` | **still broken** | none |
| R9 | `PlatformProcessTreeBackend` missing `Default` | **still broken** | #1136 |

Per-item evidence:

**R4** `src-tauri/testability/window_info.rs`. `WindowInfoOutput` (45-53), `WindowSnapshot` (55-64) and `WindowRect` (66-75) are declared unconditionally and private. Their only constructors are at 111, 158 and 163, all inside `#[cfg(target_os = "windows")] mod windows_impl` (77-235). `use serde::Serialize;` at line 2 has no other user in the file.

**R5** `src-tauri/src/config/projects.rs`. Variants at 751, 753 and 755. Sole constructors at 945, 949 and 952, inside the `#[cfg(windows)]` block at 941-954 of `validate_wire_component`. Sole assertions at 3279-3345, inside `#[cfg(windows)] mod codec_windows` at 3203. `RelDecodeError` is `pub(crate)` at 735 and a repository-wide grep finds it in this file only. It has no `impl` block and no exhaustive `match`, so removing variants per platform cannot break a match arm.

**R6** `src-tauri/src/loops/non_stop_watchdog.rs`. Field at 80. Reads: line 288 inside `#[cfg(target_os = "windows")] fn play_alarm_beep` (278), and lines 566 and 578 inside `#[cfg(test)] mod tests` (368), which carries **no** platform gate. The struct (73) derives `Clone, Default`, and a grep for a `NonStopWatchdogState { ` struct literal returns nothing; construction goes through `new()` (86) delegating to `Self::default()`.

**R7** `src-tauri/src/commands/entity_creation.rs`. Function at 3281 is already `#[cfg(test)]` (3280). Its sole caller is line 6861, inside the `#[cfg(windows)]` arm (6844) of test `delete_workgroup_dir_backend_returns_blockers_json` (6837); the `#[cfg(not(windows))]` arm (6839-6842) returns immediately.

**R8** `src-tauri/src/session/selection.rs`. `live_for_test` (217), `dormant_for_test` (222), `none_for_test` (234), each already `#[cfg(test)]`. Sole callers are `src-tauri/src/screenshot/windows.rs:1505-1507`. `screenshot/mod.rs:25-26` declares that module as `#[cfg(target_os = "windows")] mod windows;`.

**R9** `src-tauri/src/resource_monitor/windows.rs`. The `#[cfg(windows)] mod platform` (6) defines `impl Default for PlatformProcessTreeBackend` at 29-33. The `#[cfg(not(windows))] mod platform` (573) defines the same struct at 577 and `pub fn new()` at 580 but **no** `Default`, so `clippy::new_without_default` fires only off Windows. Both modules are re-exported through `pub use platform::PlatformProcessTreeBackend;` (634).

A sweep of every other `#[cfg(not(windows))]` / `#[cfg(not(target_os = "windows"))]` module in `src-tauri/src` found no second `new_without_default` candidate: `screenshot/unsupported.rs` declares no `pub fn new`, and `config/projects.rs::codec_posix` (3023) is a test module.

### 2.4 The workflow file conflicts, and the conflict is trivial

Both sides of `.github/workflows/pr-regression-gates.yml` are pure insertions at the same anchor, between `rust-regression` and `windows-release-cli-smoke`:

- `main` inserted `terminal-snapshot-portable` (94-122), a four-way matrix over `windows-latest`, `ubuntu-latest`, `macos-15`, `macos-15-intel`, running `cargo test --locked -p terminal-snapshot-renderer` and `-p session-bridge`.
- the branch inserted `rust-regression-linux` (87-132) and `rust-regression-macos` (134-174).

Neither side modified the other's jobs. The semantic resolution is a union.

---

## 3. Design decisions

Answers to the four questions in the plan request are given inline and summarised in section 3.8.

### 3.0 Governing rule

`cfg` over `allow` wherever the symbol is Windows-only in fact. **No `#[allow(dead_code)]` and no `#[allow(...)]` of any kind is added anywhere by this plan.** Every one of R4 through R9 is Windows-only in fact, so none of them meets the "legitimately cross-platform and only conditionally used" bar that would justify `allow`. This also preserves the predecessor plan's rule that no allow attributes are added.

### 3.1 R4, window-info payload types (#1134)

File `src-tauri/src/testability/window_info.rs`.

1. Add `#[cfg(target_os = "windows")]` above `struct WindowInfoOutput` (above its `#[derive(...)]`, currently line 45).
2. Add `#[cfg(target_os = "windows")]` above `struct WindowSnapshot` (currently line 55).
3. Add `#[cfg(target_os = "windows")]` above `struct WindowRect` (currently line 66).
4. Add `#[cfg(target_os = "windows")]` above `use serde::Serialize;` (line 2).

Step 4 is mandatory and easy to miss. Those three structs are the only users of `Serialize` in the file, so gating them without gating the import produces `unused_imports`, which `-D warnings` denies. Leave `use clap::Args;` (line 1) ungated: `WindowInfoArgs` (5) is unconditional. There is no `use serde_json` to gate; both `execute` arms reach it by full path.

Spelling: `target_os = "windows"` to match `mod windows_impl` (77) and the two `execute` arms (7, 33) in the same file.

### 3.2 R5, portable-path decode errors (#1132)

File `src-tauri/src/config/projects.rs`.

Add `#[cfg(windows)]` between the doc comment and the variant, for each of the three variants:

```rust
    /// A Windows-illegal character (`< > : " | ? *` or a control char).
    #[cfg(windows)]
    IllegalWindowsChar,
    /// A reserved DOS device basename (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    #[cfg(windows)]
    ReservedDosName,
    /// A component ending in a space or dot (Win32 strips these).
    #[cfg(windows)]
    TrailingDotOrSpace,
```

`#[cfg]` on an enum variant is stable and supported. This is safe here specifically because section 2.3 R5 established that the type is single-file, `pub(crate)`, has no `impl`, and is never matched exhaustively. Spelling: bare `windows` to match the producer block at 941 and the test module at 3203.

Do not gate the enum itself, and do not touch the other eight variants: they are constructed on every platform (893-923, 939).

### 3.3 R6, beep serialization state (#1133)

File `src-tauri/src/loops/non_stop_watchdog.rs`.

Add above the `beep_serialize` field (currently line 80, below its existing doc comment):

```rust
    #[cfg(any(target_os = "windows", test))]
```

The `test` disjunct is required, not decorative. `#[cfg(target_os = "windows")]` alone would make the Linux and macOS `lib test` target fail to compile, because `mod tests` (368) is not platform-gated and reads the field at 566 and 578. This predicate is exactly what #1133's title asks for: "scope beep serialization state to Windows **and watchdog tests**".

`#[derive(Clone, Default)]` on the struct is unaffected: both derives are generated over whichever fields survive `cfg` expansion, and section 2.3 R6 confirmed no struct literal exists that would need a matching field list.

### 3.4 R7, locked-file CLI test helper (#1135)

File `src-tauri/src/commands/entity_creation.rs`, line 3280.

```
-#[cfg(test)]
+#[cfg(all(test, windows))]
 pub(crate) async fn delete_workgroup_dir_backend(
```

Leave `delete_workgroup_dir_backend_with_outcome` (3295) alone: it is production code called on every platform. Leave the test at 6837 alone: its `#[cfg(not(windows))]` early return already makes it a no-op off Windows, and its `#[cfg(windows)]` arm is the only caller. Spelling: bare `windows` to match the arms inside the test.

### 3.5 R9, non-Windows process-tree `Default` (#1136)

File `src-tauri/src/resource_monitor/windows.rs`. This is the one item in the inventory that is **not** a gating problem: it is genuinely missing code, and the fix is to add it, mirroring the Windows module byte for byte.

Insert immediately after `pub struct PlatformProcessTreeBackend;` (currently line 577) and before `impl PlatformProcessTreeBackend` (579):

```rust
    impl Default for PlatformProcessTreeBackend {
        fn default() -> Self {
            Self::new()
        }
    }
```

This is a verbatim copy of lines 29-33. The lint is `clippy::new_without_default`. Do not instead derive `Default`, and do not delete `new()`: the Windows sibling establishes the `new()` plus explicit-`Default` shape, and `new()` is called by the module's own test at 627.

### 3.6 R8, selection test constructors (no issue yet)

File `src-tauri/src/session/selection.rs`, lines 216, 221 and 233.

```
-    #[cfg(test)]
+    #[cfg(all(test, target_os = "windows"))]
```

applied to each of `live_for_test`, `dormant_for_test` and `none_for_test`. Spelling: `target_os = "windows"` to match `screenshot/mod.rs:25`, which gates the sole consumer.

### 3.7 macOS `any_thread` (no issue yet)

This is the `cargo check` blocker and the only item that needs new structure.

**Rejected alternatives.**

- `#[allow(...)]`: irrelevant. `E0599` is a resolution error, not a lint.
- Per-site `let`-rebinding with a repeated `#[cfg]`: correct but duplicates Tauri's platform predicate 17 times across 11 files. Any future Tauri change means 17 edits.
- A `#[cfg(test)] mod test_support` inside the lib: covers the 13 `lib test` sites but **cannot** reach the 4 integration-test sites, because integration tests are separate crates and never see the library's `#[cfg(test)]` items.
- A `#[cfg(test)]` lib module plus a duplicate `src-tauri/tests/common/mod.rs`: covers everything, but writes the platform predicate twice, in two files that must then stay in sync. That is the exact drift risk the helper exists to remove.

**Chosen: one always-compiled helper, `#[doc(hidden)] pub`, in a new leaf module.**

Create `src-tauri/src/test_support.rs`:

```rust
//! Test-only construction helpers shared by this crate's unit tests and by the
//! integration tests under `src-tauri/tests/`.
//!
//! This module is `pub` rather than `#[cfg(test)]` on purpose: integration
//! tests are separate crates and cannot see a library's `#[cfg(test)]` items,
//! so a gated module could not serve them. It is `#[doc(hidden)]` and must not
//! be referenced from any production code path.

/// A `tauri::Builder` that tests can `build()` off the main thread wherever the
/// platform allows it.
///
/// `Builder::any_thread` is declared `#[cfg(any(windows, target_os = "linux"))]`
/// in Tauri (`tauri-2.10.3/src/app.rs:1503`): macOS requires the event loop on
/// the main thread, so the method is not exposed there. Tests that only need a
/// built `App` still compile on macOS; only the off-main-thread relaxation is
/// unavailable.
#[cfg(any(windows, target_os = "linux"))]
#[doc(hidden)]
pub fn test_builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default().any_thread()
}

/// macOS counterpart. See the note above.
#[cfg(not(any(windows, target_os = "linux")))]
#[doc(hidden)]
pub fn test_builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
}
```

Two whole function definitions rather than one function with an internal `#[cfg]` `let`, for two reasons. First, it is this repository's established idiom for platform-split functions: `window_info.rs:7,33` (`execute`), `non_stop_watchdog.rs:278,309` (`play_alarm_beep`), `resource_monitor/windows.rs:6,573` (`mod platform`). Second, the internal-`let` form degenerates on macOS to `let builder = ...; builder`, which trips `clippy::let_and_return`, a warn-by-default style lint that `-D warnings` would turn into a hard failure. That would replace one macOS break with another.

Register it in `src-tauri/src/lib.rs` by inserting one line immediately before `pub mod testability;` (currently line 18):

```rust
pub mod test_support;
```

**Call-site rewrite, 17 sites in 11 files.** Every site currently reads:

```rust
tauri::Builder::default()
    .any_thread()
```

Replace both lines with a single line:

- in `src-tauri/src/**`: `crate::test_support::test_builder()`
- in `src-tauri/tests/**`: `agentscommander_lib::test_support::test_builder()`

preserving each site's existing indentation and the method chain that follows. The exact site list is the "fix" rows of the table in section 2.2. Do **not** touch `src-tauri/tests/pty_lifecycle_regression.rs`.

**Accepted consequence, stated explicitly.** On macOS the returned builder is not `any_thread`, so a test that calls `.build(...)` off the main thread would panic **at run time** on macOS. That is acceptable and is not hidden: Tauri offers no macOS alternative, and neither new gate runs `cargo test`. `cargo test` on macOS remains out of scope for #1154 and is not enabled by this plan. If macOS test execution is ever added, these tests will need `#[cfg(not(target_os = "macos"))]` or a main-thread harness, and that is a separate issue.

**Accepted consequence, production surface.** An eight-line `#[doc(hidden)] pub fn` is compiled into the release library. It is `pub`, so it raises no `dead_code` lint; it is never called from production code; it costs nothing after LTO. Section 5 criterion 12 pins that it stays unreferenced by production code.

### 3.8 Direct answers to the plan request's four questions

1. **`cfg` versus `allow`.** `cfg` in every case. Zero `#[allow]` attributes added. No symbol in this inventory is legitimately cross-platform, so the `allow` escape hatch is never reached. Predicates chosen per item: `target_os = "windows"` (3.1, 3.6), `windows` (3.2, 3.4), `any(target_os = "windows", test)` (3.3), `all(test, windows)` (3.4), `all(test, target_os = "windows")` (3.6). Each spelling matches the gate already used on that symbol's consumer in the same file or module, so no file mixes spellings for the same concept.
2. **`any_thread`.** Exact shape `#[cfg(any(windows, target_os = "linux"))]`, verified against `tauri-2.10.3/src/app.rs:1503`. It goes in exactly one place, on a pair of `test_builder` definitions in a new `src-tauri/src/test_support.rs`, and 17 call sites are rewritten to call it. Full detail in 3.7.
3. **Is anything genuinely dead and deletable?** No. Zero deletions. Every symbol has a live, verified Windows consumer: R4 constructed at `window_info.rs:111,158,163`; R5 constructed at `projects.rs:945,949,952` and asserted at `3279-3345`; R6 read at `non_stop_watchdog.rs:288,566,578`; R7 called at `entity_creation.rs:6861`; R8 called at `screenshot/windows.rs:1505-1507`; R9 is a missing impl on a type exported at `resource_monitor/windows.rs:634`. Every fix is a `cfg` narrowing or one added impl.
4. **Files, symbols, order, criteria.** Sections 4 and 5.

### 3.9 Two things deliberately not changed

- **`--workspace` asymmetry.** `main`'s Windows `rust-regression` runs `cargo clippy --workspace --all-targets`; the two new jobs run `cargo clippy --all-targets`. Left as authored. Adding `--workspace` would newly gate `crates/session-bridge` and `crates/terminal-snapshot-renderer` on Linux and macOS for the first time, expanding blast radius well beyond "make the two gates pass", and `terminal-snapshot-portable` already exercises both crates on `ubuntu-latest`, `macos-15` and `macos-15-intel`. This is not a weakening: the commands are exactly the ones #1154 acceptance criterion 2 fixed.
- **Rust cache path.** The two new jobs use `workspaces: src-tauri -> target`, which is wrong because the workspace root is the repository root and `src-tauri/target` never exists. It is a cache-efficiency defect, never a correctness one. It was already recorded as out-of-scope finding 1 of the predecessor plan, where it applies equally to `rust-regression`, `windows-release-cli-smoke` and `release.yml`. Fixing it here would require touching existing jobs, which #1154 acceptance criterion 8 forbids. Leave it; keep the existing follow-up.

---

## 4. Implementation order

Each step is independently verifiable. Do not reorder: step 1 changes which of the later steps are still needed.

### Step 1. Bring the branch onto current `main` (mandatory first)

```
git fetch origin main
git checkout ci/1154-nonwindows-rust-gates
git merge origin/main
```

Merge, not rebase. The branch is published and the PR must keep its identity and its single authored commit; rebasing 342 commits under a force-push discards the review history for no benefit.

Exactly one conflict is expected, in `.github/workflows/pr-regression-gates.yml`. Resolve it as a **union with no content edits to any job**, producing this job order:

```
test-debt
rust-regression
rust-regression-linux
rust-regression-macos
terminal-snapshot-portable
windows-release-cli-smoke
frontend-regression
```

`rust-regression-linux` and `rust-regression-macos` keep the branch's bytes; `terminal-snapshot-portable` keeps `main`'s bytes; every other job and the workflow-level `name`, `on`, `permissions` and `concurrency` blocks keep `main`'s bytes. Keeping the three `rust-regression*` jobs contiguous preserves the intent stated in the branch's own commit message.

If any conflict appears outside this one file, stop and report: it means `main` moved again between this plan's base and the merge.

After the merge, verify that R1 through R3 arrived: `src-tauri/src/commands/wg_delete_diagnostic.rs` must contain `#[cfg(windows)]` above `const MAX_FILES_PER_PROCESS` and `#[cfg(any(windows, test))]` above `fn win32_meaning` and `fn win32_error`. If not, `main` is not what this plan was written against; stop and report.

### Step 2. Record the Windows baseline before touching any source

```
cd src-tauri
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
```

Both must be clean on the merge result **before** any edit from step 3. If either is already red, stop and report: the Windows regression criterion is unmeasurable until that is explained.

### Step 3. Apply the source fixes

Apply 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, then 3.7, in that order. The first six are one to four attribute lines each and are mutually independent. 3.7 is last because it is the only one that adds a file and touches 11 files.

### Step 4. Re-verify Windows

Repeat step 2's two commands. Both must still be clean. Then run:

```
cargo fmt
git diff --stat
```

`cargo fmt` must produce no change outside the files listed in section 4.1. If it reformats anything else, revert that hunk: this plan does not carry a formatting sweep.

Also run, from the repository root, `npm run test:debt`, which must stay green.

### Step 5. Regenerate the module arc record

From the repository root:

```
npm run record:arcs
```

`src-tauri/module-arcs.txt` must change by exactly the added `-> agentscommander_lib::test_support` arcs and nothing else. See section 6.

### Step 6. Push and iterate against CI (expected, bounded, not a failure)

Commit and push to `ci/1154-nonwindows-rust-gates`, then read the run.

This loop is expected because 342 commits of Rust have never been compiled for Linux or macOS by any CI run. Sections 2.2 and 2.3 are the complete inventory of *known* defects, verified individually; they are not a guarantee that no *unknown* defect exists in code that landed after `1e7f2350`.

Loop rules:

- **Bound: three push-and-read iterations.** If the gates are not green after the third, stop and report the remaining diagnostics rather than continuing.
- Each iteration fixes only diagnostics reported by that run. Do not pre-emptively gate symbols CI has not flagged.
- Every new fix must follow section 3.0: `cfg` narrowing where the symbol is platform-specific in fact, a real implementation where something is genuinely missing, never `#[allow]`, never `continue-on-error`, never dropping `-D warnings`, never `|| true`.
- After each iteration, re-run step 4 before pushing again. A Linux or macOS fix that breaks Windows is a regression, not progress.
- If a diagnostic cannot be resolved without an `#[allow]`, stop and report it as an open decision instead of adding one.

---

### 4.1 Complete file change matrix

| File | Change | Lines touched |
|---|---|---|
| `.github/workflows/pr-regression-gates.yml` | merge-conflict resolution, union of both insertions | conflict region only |
| `src-tauri/src/testability/window_info.rs` | 4 `#[cfg(target_os = "windows")]` added | +4 |
| `src-tauri/src/config/projects.rs` | 3 `#[cfg(windows)]` added on enum variants | +3 |
| `src-tauri/src/loops/non_stop_watchdog.rs` | 1 `#[cfg(any(target_os = "windows", test))]` added | +1 |
| `src-tauri/src/commands/entity_creation.rs` | `#[cfg(test)]` -> `#[cfg(all(test, windows))]` | 1 modified |
| `src-tauri/src/resource_monitor/windows.rs` | `impl Default` added to the `not(windows)` module | +5 |
| `src-tauri/src/session/selection.rs` | 3x `#[cfg(test)]` -> `#[cfg(all(test, target_os = "windows"))]` | 3 modified |
| `src-tauri/src/test_support.rs` | **new file** | +~25 |
| `src-tauri/src/lib.rs` | `pub mod test_support;` added; 1 call site rewritten | +1, 2 modified |
| `src-tauri/src/commands/ac_discovery.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/commands/config.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/commands/session.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/loops/delivery.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/pty/local_backend.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/src/web/commands.rs` | 5 call sites rewritten | 10 -> 5 |
| `src-tauri/src/web/event_broadcast.rs` | 1 call site rewritten | 2 -> 1 |
| `src-tauri/tests/pty_powershell_managed_native.rs` | 2 call sites rewritten | 4 -> 2 |
| `src-tauri/tests/wake_consumption_measure.rs` | 2 call sites rewritten | 4 -> 2 |
| `src-tauri/module-arcs.txt` | regenerated | see section 6 |
| `plans/1154-nonwindows-rust-gate-failures.md` | this plan, `git add -f` | new |

No other file may change. In particular `src-tauri/tests/pty_lifecycle_regression.rs`, `crates/**`, `src/**`, `Cargo.toml`, `Cargo.lock` and `package.json` are untouched.

`plans/` is ignored by root `.gitignore`, so this plan must be force-added with `git add -f`, exactly as the predecessor plan was. Do not weaken the ignore rule.

---

## 5. Objective acceptance criteria

This change is acceptable only when every statement is true.

1. `git merge-base HEAD origin/main` equals `origin/main`, that is, the branch contains current `main`, and `gh api repos/mblua/AgentsCommander/pulls/1156 --jq .mergeable` returns `true`.
2. `.github/workflows/pr-regression-gates.yml` defines all seven jobs in the order given in step 1, and `git diff origin/main -- .github/workflows/pr-regression-gates.yml` shows insertions only, zero deletions, and no change to `test-debt`, `rust-regression`, `terminal-snapshot-portable`, `windows-release-cli-smoke`, `frontend-regression`, or to the workflow-level `name`, `on`, `permissions` and `concurrency` blocks.
3. `rust-regression-linux` and `rust-regression-macos` still run `cargo check --all-targets` then `cargo clippy --all-targets -- -D warnings` with `working-directory: src-tauri`, and neither runs `cargo test`. No `continue-on-error`, no `|| true`, no relaxation of `-D warnings`.
4. On Windows, from `src-tauri`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` both exit 0.
5. `grep -rn "allow(dead_code)" src-tauri/src src-tauri/tests` returns no line introduced by this change, and the change adds no `#[allow(...)]` attribute of any kind.
6. `grep -rn "\.any_thread()" src-tauri/` returns exactly three lines: one in `src-tauri/src/test_support.rs`, plus the two pre-existing lines at `src-tauri/tests/pty_lifecycle_regression.rs:285,313`. No other file in `src-tauri/` calls `.any_thread()`. (A plain `grep -rn "any_thread"` additionally matches the doc comment in `test_support.rs`, which is expected.)
7. `src-tauri/src/test_support.rs` exists, declares exactly two `test_builder` definitions under `#[cfg(any(windows, target_os = "linux"))]` and its negation, and `src-tauri/src/lib.rs` declares `pub mod test_support;`.
8. Each of the six residue fixes is present with the exact predicate specified in sections 3.1 through 3.6, and no symbol was deleted.
9. `npm run test:debt` passes, and `cargo fmt` produces no diff outside section 4.1's file list.
10. The committed file set equals section 4.1 exactly, with this plan force-added.
11. On the pull request: `rust-regression-linux` and `rust-regression-macos` both conclude **success**, and `rust-regression`, `frontend-regression`, `test-debt`, `lockfile-drift`, `windows-release-cli-smoke`, `validate-branch-name` and `terminal-snapshot-portable` all still conclude success.
12. `grep -rn "test_support" src-tauri/src` shows references only from `lib.rs`'s module declaration and from code inside `#[cfg(test)]` modules. No production code path references it.
13. Section 6's dependency-cycle criterion holds.

---

## 6. Dependency-cycle gate

Applied per the `verify-no-dependency-cycles` skill. Verdict: **PASS, no new cycle, no SCC growth, no cross-boundary arc.**

**Arcs removed:** none.

**Arcs added:** nine, all pointing at the new leaf `agentscommander_lib::test_support`:

```
agentscommander_lib -> agentscommander_lib::test_support
agentscommander_lib::commands::ac_discovery -> agentscommander_lib::test_support
agentscommander_lib::commands::config -> agentscommander_lib::test_support
agentscommander_lib::commands::session -> agentscommander_lib::test_support
agentscommander_lib::loops::delivery -> agentscommander_lib::test_support
agentscommander_lib::pty::local_backend -> agentscommander_lib::test_support
agentscommander_lib::pty::terminal_snapshot -> agentscommander_lib::test_support
agentscommander_lib::web::commands -> agentscommander_lib::test_support
agentscommander_lib::web::event_broadcast -> agentscommander_lib::test_support
```

(The `pty::terminal_snapshot` arc is attributed to that module because `acceptance_tests` is its `#[cfg(test)]` submodule. Whether the recorder emits it under the parent or the submodule name does not affect the analysis.)

**Classification.** `test_support` has intra-crate out-degree **zero**: its only dependency is the external `tauri` crate. A node with no outgoing intra-crate arc cannot lie on any cycle, so none of the nine arcs can close a cycle, merge two SCCs, or grow one. Therefore `cyclicSccs` is unchanged and every SCC member set is identical. Every added arc terminates at a brand-new node, so no arc crosses a previously-clean SCC boundary.

Sections 3.1 through 3.6 add or narrow `cfg` attributes and add one `impl Default` inside an existing module. They introduce no module-to-module reference whatsoever, so they contribute no arc in either direction.

The `src-tauri/tests/**` edits are separate crates and are not part of the library's module graph.

**Role and layering hygiene.** `test_support` returns a `tauri::Builder`, so it is UI-transport-typed. It is a **new top-level leaf**, so no pre-existing lower-layer module gains a transport dependency it did not already have: all eight referencing modules already construct `tauri::Builder` today at the very lines being rewritten. No pure predicate is moved below a transport boundary, and no transport-taking function is pushed downward.

**Step-N detector acceptance criterion.** After step 5, `git diff -- src-tauri/module-arcs.txt` must show **exactly** the nine lines above as additions, in the file's existing sort order, and **zero** other added, removed or modified lines. Any other delta means an unintended module reference was introduced; stop and report rather than accepting it.

---

## 7. Open items for the coordinator

Listed for decision. None of them blocks implementation of sections 3 through 6.

1. Whether PR #1156 may absorb #1132 through #1136, per section 1. If the answer is no, sections 3.1 through 3.6 lift out onto five per-issue branches unchanged, and this branch keeps only step 1, section 3.7 and the workflow merge.
2. Filing `B14-SELECTION-TEST-CFG` and `B15-TAURI-ANY-THREAD-MACOS` as children of #1113 for sections 3.6 and 3.7.
3. Updating epic #1113's checklist and the affected child issue bodies to record where the work actually landed.
4. Whether `cargo test` should ever run on macOS, which would require revisiting section 3.7's accepted run-time consequence.
