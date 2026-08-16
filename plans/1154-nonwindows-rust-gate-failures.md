# Plan #1154-fix: make `rust-regression-linux` and `rust-regression-macos` pass on PR #1156

Author: architect, wg-8. Authored and certified in a single Lite pass on 2026-08-15 UTC.
Amendment 1 authored and recertified on 2026-08-16 UTC after the first two CI iterations.

Status: READY_FOR_IMPLEMENTATION

Certification note: `Plan-SHA256` is the SHA-256 of this file's exact bytes as written. No hash line is embedded in the file, so the value is reproducible with `Get-FileHash -Algorithm SHA256` on the committed path.

Issue: [mblua/AgentsCommander#1154](https://github.com/mblua/AgentsCommander/issues/1154).
Pull request: [#1156](https://github.com/mblua/AgentsCommander/pull/1156), base `main`, head `1deb4e77d9fe3786af54878cb18e168cf0416931` at first certification, `6c792b3f` at Amendment 1.
Branch: `ci/1154-nonwindows-rust-gates`.
Predecessor plan: `plans/1154-nonwindows-rust-gates.md` on the same branch.

---

## Amendment 1 (2026-08-16): read this before section 0

Sections 0 through 7 are the original plan and are **kept verbatim except where this
amendment says otherwise**, because they remain the record of what was designed and why.
Everything the first pass specified was implemented in commits `9c7c77e5` and `6c792b3f`,
and every defect in the section 2 inventory is gone: macOS clears `cargo check`, and both
non-Windows jobs now reach `cargo clippy`.

Two things then happened that this amendment rules on.

1. **A layering guard fired on Windows.** `src-tauri/tests/project_settings_layering.rs`
   pins the exact set of crate modules that `web::event_broadcast` may name. Section 3.7's
   call-site rewrite added `test_support` to that set and broke
   `the_emitter_home_names_nothing_but_the_websocket_fan_out`. Section 3.7 is amended by
   **section 3.10**, section 6 is replaced by **section 6 (Amendment 1)**, and section 4
   gains a mandatory local guard-test step.
2. **A second non-Windows frontier appeared**, ruled on per symbol in **sections 3.11
   through 3.16**. It is not all dead-code gating: several items are ordinary clippy
   findings in `#[cfg(unix)]` code that Windows never compiles.

Amendment 1 also revises the governing rule (**section 3.0a**), the iteration budget
(**section 4, step 6, amended**), the file matrix (**section 4.1 (Amendment 1)**), the
acceptance criteria (**section 5 (Amendment 1)**) and adds a written third-frontier
assessment (**section 8**).

Base of every fact in Amendment 1: working tree at branch head `6c792b3f`, read directly.
Codebase Memory gate: `ready`, project
`D-0_repos-AgentsCommander_iac-.ac-wg-8-dev-v5-team-repo-AgentsCommander`, 23914 nodes,
128566 edges, indexed head `6c792b3f`.

**Evidence preserved.** dev-rust's uncommitted edits in
`src-tauri/src/web/event_broadcast.rs` and
`src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` were read, not modified. Both
are **ratified as written** (sections 3.10 and 3.16). Nothing in this amendment asks for
them to be reverted or rewritten.

**Measurement limitation, stated up front.** This amendment was authored on Windows. No
Linux or macOS toolchain was available, so every ruling below is derived from reading the
source, its `cfg` gates and its consumers, not from running `cargo clippy` on those
platforms. Where the tech lead's diagnostic counts (17 in `lib`, 19 in `lib test`) cannot
be reconciled item by item with the enumerated list, this amendment says so rather than
inventing the difference; see section 5 (Amendment 1) criterion A6, which is written as
"zero diagnostics", never as "these N are fixed".

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

### 3.0a Governing rule, amended (Amendment 1)

The rule above survives for every dead-code item, and Amendment 1 adds nothing to the
`#[allow(dead_code)]` count: criterion 5 still holds unchanged.

It does **not** survive `clippy::enum_variant_names` untouched, and the tech lead is right
to demand an explicit ruling rather than leaving it to be inferred. The amended rule is:

> `cfg` where the symbol is platform-specific in fact. A real code change where the lint
> names a real defect. **Exactly one `#[allow(...)]` is permitted by this plan, the one
> named in section 3.12, and no other.** Any further diagnostic that appears to need an
> `#[allow]` is a stop-and-report, not a decision the implementer may take.

Three facts support carving out that one exception rather than bending the rest of the plan
around it.

1. `#[allow(clippy::...)]` with a written reason is already this repository's idiom, not a
   novelty: `#[allow(clippy::too_many_arguments)]` appears 56 times under `src-tauri/src`,
   `#[allow(clippy::await_holding_lock)]` 12 times, `#[allow(private_interfaces)]` 5 times.
   There is no `clippy.toml` and no `[lints]` table, so the lint set is stock clippy.
2. `#[expect(...)]` is deliberately **not** used. It appears zero times in the repository,
   and introducing it here would make this plan the place a new attribute style enters the
   codebase, which is out of scope.
3. The one exception sits on an item that is already `#[cfg(unix)]`, so it changes zero
   Windows-compiled bytes.

Two rules that were implicit in the first pass and are now explicit, because the second
frontier is where they start to matter.

- **Second-order rule.** Narrowing a `cfg` can orphan the thing that used to reference the
  narrowed symbol. Every ruling below states its cascade, and section 3.13 exists entirely
  because of one. An orphan found at implementation time that this plan did not predict is
  a stop-and-report.
- **Windows-invariance rule.** Every ruling below states, in its own words, whether it
  changes any byte the Windows build compiles. Where the answer is "none", that is the
  strongest available answer to "confirm it cannot regress Windows" and is preferred over
  an equivalent fix that touches shared code.

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

**Call-site rewrite, 17 sites in 11 files.** *(Amendment 1: 16 sites in 10 files.
`src/web/event_broadcast.rs:41` is struck from this list and ruled on separately in
section 3.10. Everything else in this section stands as written and is already
implemented.)* Every site currently reads:

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

### 3.10 The emitter home: ratify the inline predicate, leave #1265 alone (Amendment 1)

**Ruling: dev-rust's uncommitted resolution is ratified exactly as it stands on disk.
`ALLOWED_EMITTER_CRATE_REFERENCES` stays untouched at one row. Nothing here enlarges the
#1265 knot, and section 4.3 of `plans/1265-extract-project-settings-from-scc.md` is not
rewritten.**

### 3.10.1 What actually broke, verified

`src-tauri/tests/project_settings_layering.rs` holds three equalities over
`src/web/event_broadcast.rs`, one per anchor:

| Anchor | Table | Line | Allowed set |
|---|---|---|---|
| `crate::` | `ALLOWED_EMITTER_CRATE_REFERENCES` | 215 | `{("src/web/event_broadcast.rs", "web")}` |
| `web::` | `ALLOWED_EMITTER_WEB_REFERENCES` | 228 | `{("src/web/event_broadcast.rs", "broadcast")}` |
| `super::` | `ALLOWED_EMITTER_SUPER_REFERENCES` | 250 | `{("src/web/event_broadcast.rs", "broadcast_all")}` |

`children_under` (542) scans for the anchor text anywhere in the scrubbed, normalized file
body. It is **not** restricted to `use` items, so an expression path counts: writing
`crate::test_support::test_builder()` inside `mod tests` reports the child `test_support`
under the `crate::` anchor, and the equality at 1264 fails. That is exactly the observed
failure. Comments and string literals are removed by `scrub` (300) before the scan, which
is why the explanatory comment dev-rust wrote does not itself re-trip the guard.

This is a **naming-set property, not a reachability property.** Section 6 of the first pass
concluded PASS on the correct but insufficient grounds that `test_support` has intra-crate
out-degree zero and therefore cannot lie on a cycle. That reasoning is still true and is
still the right answer to the cycle question. It simply does not answer the question this
guard asks, which is "what may this module name at all". Section 6 (Amendment 1) fixes the
gate so this class is checked in future.

### 3.10.2 Why the allowlist row is refused

Adding `("src/web/event_broadcast.rs", "test_support")` would pass CI and would not, today,
create a cycle. It is refused anyway, for three reasons.

1. **It is not this plan's decision to take.** The test's own message says a new row "is a
   decision about the crate's shape, not a detail", and that section 4.3 of the #1265 plan
   "has to be rewritten before the dependency is added". #1154 is a CI-gate repair. It has
   no mandate to amend another plan's specification, and the tech lead explicitly asked me
   to stop rather than enlarge the knot.
2. **It converts a proved invariant into a conditional one.** Today the non-absorption
   argument is "this module's whole in-crate dependency is `WsBroadcaster`". With the row it
   becomes "that, plus `test_support`, for as long as `test_support` stays a leaf". Nothing
   in the repository enforces the second clause. `test_support` is `pub`, always compiled,
   and one `use crate::…` away from making the emitter reach the knot, at which point
   `commands::project_settings` returns to the SCC and, per the test's own measurement, the
   knot ends up **larger** than before #1265.
3. **The cost of refusing is one call site.** There is no shared benefit being given up.

### 3.10.3 The ratified resolution

`src-tauri/src/web/event_broadcast.rs`, inside `#[cfg(test)] mod tests`, at the site that
section 3.7 had rewritten. On disk now, and to be committed unchanged:

```rust
        // Deliberately NOT `crate::test_support::test_builder()`. This file is
        // the #1265 emitter home, and `the_emitter_home_names_nothing_but_the_
        // websocket_fan_out` pins the exact set of crate modules it may name.
        // Tauri declares `Builder::any_thread` only for Windows and Linux, so
        // the predicate is written out here rather than imported.
        #[cfg(any(windows, target_os = "linux"))]
        let builder = tauri::Builder::default().any_thread();
        #[cfg(not(any(windows, target_os = "linux")))]
        let builder = tauri::Builder::default();
        let app = builder
```

Verified properties of this exact text:

- It introduces **no** `crate::`, `web::` or `super::` anchor occurrence, so all three
  emitter equalities return to their pre-#1154 values and the test goes green with the
  tables untouched.
- The predicate is byte-identical to Tauri's own (`tauri-2.10.3/src/app.rs:1503`), the same
  one `test_support::test_builder` carries, so the two cannot drift apart silently: section
  5 (Amendment 1) criterion A2 pins that they stay equal.
- `let builder = …; let app = builder.manage(…)` is not a `let`-and-return, so
  `clippy::let_and_return` cannot fire. This is the one shape section 3.7 rejected for the
  helper, and it is safe here only because the binding is used, not returned.
- Windows-invariance: on Windows the first arm is selected and produces
  `tauri::Builder::default().any_thread()`, which is what the file compiled before #1154.

**Accepted cost, stated plainly.** Tauri's platform predicate is now written in two places,
`test_support.rs` and this one call site. Section 3.7 rejected a two-place duplication when
the alternative was one place covering everything; here the alternative is amending another
plan's specification, which is worse. One duplicated predicate, pinned by an acceptance
criterion, is the cheaper of the two.

**Consequences for the rest of the plan**, all folded in below: section 3.7's site count
becomes 16 in 10 files; section 4.1 loses `event_broadcast.rs` from the call-site rows and
gains it as an amendment row; section 6's nine-arc criterion is replaced.

---

### 3.11 The #1271 window-screenshot surface: complete an existing, documented gate (Amendment 1)

Eight of the tech lead's dead-code items and the `api/error.rs` item are one feature: the
native window-screenshot path of #1271. `src/api/handlers/mod.rs:60` declares
`#[cfg(target_os = "windows")] pub(crate) mod window_screenshot;`, so the entire consumer
side of that feature exists only on Windows. Every symbol below is therefore Windows-only
in fact, and `cfg` is the right instrument, exactly as for R4 through R9.

**This is not a new pattern. It is an unfinished one.** `src/api/mod.rs:45-61` already gates
the sibling limiter surface of the same feature and writes down the reasoning:

> The native window-screenshot capture path exists only on Windows
> (`handlers::window_screenshot` and `screenshot::windows` are both gated on it), so the
> limiter that bounds it is scoped the same way. The `test` disjunct keeps
> `window_screenshot_limiter_tests` below and the route-level queue tests in
> `pty/terminal_snapshot/acceptance_tests.rs`, neither of which is platform-gated.

That comment is the specification for sections 3.11 and 3.13. It also fixes the two
spellings used throughout `api/`: `#[cfg(any(target_os = "windows", test))]` for a symbol a
non-platform-gated test still consumes (mod.rs:51, 53, 55, 63, 88, 99, 105, 136), and bare
`#[cfg(target_os = "windows")]` for a symbol nothing off Windows consumes (mod.rs:58, 60).
Sections 3.11 and 3.13 use those two spellings and no others.

### 3.11.1 Which consumer each symbol actually has, verified

| Symbol | `src/api/audit.rs` | Consumers off the Windows handler |
|---|---|---|
| `WindowScreenshotAuditStatus` | 153 | `window_screenshot_audit_tests` at 226, **not** platform-gated (233-250, 264) |
| `WindowScreenshotAuditResult` | 162 | none; only `window_screenshot.rs:214` constructs it |
| `WINDOW_SCREENSHOT_AUDITS_FOR_TEST` | 167 | only the three `_for_test` items below and `record_…`'s `#[cfg(test)]` block at 308 |
| `WindowScreenshotAuditCaptureForTest` + its `Drop` | 172, 175 | `acceptance_tests.rs`, every call site `#[cfg(target_os = "windows")]` |
| `lock_window_screenshot_audits_for_test` | 184 | same, 12 call sites, all under `#[cfg(target_os = "windows")]` tests |
| `take_window_screenshot_audits_for_test` | 197 | same |
| `WindowScreenshotAuditMetadata` + `new` | 208, 216 | `window_screenshot_audit_tests` at 264, **not** platform-gated |
| `record_window_screenshot_result` | 307 | none; only `window_screenshot.rs:214` calls it |
| `WindowScreenshotApiError` (`src/api/error.rs`) | 14 | `window_screenshot_tests` at 23, **not** platform-gated (31-51, 58) |

Every `acceptance_tests.rs` test that touches this surface was checked individually and
carries `#[cfg(target_os = "windows")]`: lines 1253, 1305, 1365, 1811, 1938, 2061, 2203,
2317, 2452, 2643, 2841, 2868, 3033. That is why the `_for_test` helpers are dead in
`lib test` on Linux and macOS but not on Windows.

### 3.11.2 Per-symbol ruling, `src/api/audit.rs`

Insert or change exactly these attributes. Line numbers are the current ones at `6c792b3f`;
the anchor text is authoritative if they have moved.

| # | Attribute line | Applies to | Ruling |
|---|---|---|---|
| B1 | insert above 151 (`#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]`) | `enum WindowScreenshotAuditStatus` | `#[cfg(any(target_os = "windows", test))]` |
| B2 | insert above 162 (`pub(crate) struct WindowScreenshotAuditResult {`) | that struct | `#[cfg(target_os = "windows")]` |
| B3 | change 166 from `#[cfg(test)]` | the `std::thread_local!` at 167 | `#[cfg(all(test, target_os = "windows"))]` |
| B4 | change 171 from `#[cfg(test)]` | `struct WindowScreenshotAuditCaptureForTest` | `#[cfg(all(test, target_os = "windows"))]` |
| B5 | change 174 from `#[cfg(test)]` | `impl Drop for WindowScreenshotAuditCaptureForTest` | `#[cfg(all(test, target_os = "windows"))]` |
| B6 | change 183 from `#[cfg(test)]` | `fn lock_window_screenshot_audits_for_test` | `#[cfg(all(test, target_os = "windows"))]` |
| B7 | change 196 from `#[cfg(test)]` | `fn take_window_screenshot_audits_for_test` | `#[cfg(all(test, target_os = "windows"))]` |
| B8 | insert above 207 (`#[derive(serde::Serialize)]`) | `struct WindowScreenshotAuditMetadata` | `#[cfg(any(target_os = "windows", test))]` |
| B9 | insert above 215 (`impl WindowScreenshotAuditMetadata {`) | the whole `impl` block, which is the only home of `new` | `#[cfg(any(target_os = "windows", test))]` |
| B10 | insert above 307 (`pub(crate) fn record_window_screenshot_result`) | that function | `#[cfg(target_os = "windows")]` |

Why each predicate, in one line each:

- B1, B8, B9 keep the disjunct because `window_screenshot_audit_tests` (226) is not
  platform-gated and constructs `WindowScreenshotAuditStatus` at 233-250 and
  `WindowScreenshotAuditMetadata::new` at 264. Gating them strictly would break the Linux
  and macOS `lib test` compile, which is the same trap section 3.3 documented for
  `beep_serialize`. It also **keeps that test running on all three platforms**, which is the
  choice `api/mod.rs:45-50` already made for the sibling limiter tests: the properties they
  assert (fixed redacted serialization, no sensitive field in the audit line) are
  platform-independent properties of a Windows-only surface and are worth running everywhere.
- B9 gates the `impl` block rather than `fn new`, so the block and its `Self` type can never
  disagree about their gate.
- B2, B10 take the strict predicate because nothing outside the Windows handler names them,
  and a disjunct there would leave them present-and-unused in Linux `lib test`, that is, it
  would move the diagnostic rather than remove it.
- B3 through B7 add `target_os = "windows"` to the existing `test`, because every caller is
  a `#[cfg(target_os = "windows")]` test.

**Windows-invariance.** All ten predicates are satisfied on Windows, so the Windows build
compiles exactly the items it compiles today. Zero Windows-compiled bytes change.

**Cascades checked.** `record_audit_metadata` (53) keeps its other caller at 74, so B10 does
not orphan it. `use serde::Serialize;` (13) keeps its user, `PtyInputAuditMetadata` (22), so
no import is orphaned. `WindowScreenshotAuditStatus` is spelled `serde::Serialize` by full
path in its derive, so B1 orphans nothing either.

### 3.11.3 Per-symbol ruling, `src/api/error.rs`

The reported diagnostic is "variants never constructed", which fires on the `lib` target
only: `window_screenshot_tests` (23) constructs all five at 31-51, so `lib test` is clean
today. Gating the five variants individually would leave an inhabited-nowhere enum behind a
`ApiError::WindowScreenshot(_)` payload and would still need the arms gated. Gating the
whole surface is smaller and mirrors `api/mod.rs`.

| # | Attribute line | Applies to | Ruling |
|---|---|---|---|
| B11 | insert above 13 (`#[derive(Debug, Clone, Copy)]`) | `enum WindowScreenshotApiError` | `#[cfg(any(target_os = "windows", test))]` |
| B12 | insert above 73 (`#[allow(private_interfaces)]`) | the `ApiError::WindowScreenshot` variant | `#[cfg(any(target_os = "windows", test))]` |
| B13 | insert above each of 102, 107, 112, 117, 122 | the five `ApiError::WindowScreenshot(...)` arms of `parts()` | `#[cfg(any(target_os = "windows", test))]` |

Five separate attributes and not one, because an attribute on a match arm covers that arm
only. `#[cfg]` on a match arm and on an enum variant are both stable; the latter is the same
mechanism section 3.2 already used for `RelDecodeError`.

`parts()` stays exhaustive on every platform: with B12 the variant does not exist off
Windows outside `test`, and with B13 the arms that name it disappear on exactly the same
predicate. The two must therefore carry **the identical predicate string**; a mismatch is a
compile error on the platform where they disagree, which is a loud failure rather than a
quiet one.

**Blast-radius check.** A repository-wide search for `ApiError::WindowScreenshot` and
`WindowScreenshotApiError` returns only `src/api/error.rs` (13, 14, 31-58, 74, 102-122) and
`src/api/handlers/window_screenshot.rs` (22, 131-183, 205, 208, 210), and the latter file is
`#[cfg(target_os = "windows")]` in its entirety. No third module matches on `ApiError`
exhaustively, so removing a variant off Windows cannot break an unrelated match.

**Windows-invariance.** Zero Windows-compiled bytes change.

---

### 3.12 `clippy::enum_variant_names`: the one justified `#[allow]` (Amendment 1)

`src/path_identity.rs:73-79`:

```rust
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnixTrackedCleanupStage {
    BeforeClaimRename,
    BeforeRestore,
    BeforeClaimUnlink,
}
```

**Ruling: add `#[allow(clippy::enum_variant_names)]` on the enum, with the justification
comment below. Do not rename the variants.**

This is the exception section 3.0a permits and the only one this plan grants.

**Why not the rename.** Dropping the prefix gives `ClaimRename`, `Restore`, `ClaimUnlink`.
That is **15 variant-name occurrences across three files**: `src/path_identity.rs` (76, 77,
78, 336, 365, 381, 2197, 2235, 2295, 2331, 2335, 2338),
`src/pty/terminal_snapshot.rs` (3434, 3480) and `src/phone/terminal_snapshot.rs` (2263).
Every one of those lines is inside `#[cfg(unix)]`
code, which means **Windows cannot compile a single one of them**: the implementer would be
editing 21 lines with no local compiler able to check the result, and one missed site costs a
full CI iteration out of a budget of two. That risk is not worth taking for a cosmetic
rename.

**Why the prefix is not noise, which is the substantive argument.** These are the stages of
a cleanup hook, and the hook's contract is that it is called *before* the named step
happens: `hook(UnixTrackedCleanupStage::BeforeClaimRename, …)` at 336 fires before the
claim rename, `BeforeRestore` at 365 before the restore, `BeforeClaimUnlink` at 381 before
the unlink. `Restore` would name the step; `BeforeRestore` names the moment. The lint's
shared-prefix heuristic cannot see that difference, and this is precisely the false positive
clippy's own documentation warns about for `enum_variant_names`.

**Exact edit.** Between the `#[cfg(unix)]` at 73 and the `#[derive(...)]` at 74, insert:

```rust
// The `Before` prefix is the contract, not noise: each variant names the moment
// the cleanup hook fires, which is BEFORE the step it is named after. Renaming to
// `Restore` / `ClaimRename` / `ClaimUnlink` would name the steps instead and lose
// that. clippy's shared-prefix heuristic cannot distinguish the two cases.
#[allow(clippy::enum_variant_names)]
```

**Windows-invariance.** The item is already `#[cfg(unix)]`, so Windows compiles neither the
enum nor the attribute. Zero Windows-compiled bytes change.

**Consequence for criterion 5.** Criterion 5 is amended in section 5 (Amendment 1) to
"exactly one `#[allow]`, the one in section 3.12". The `#[allow(dead_code)]` half of
criterion 5 is unchanged and still holds at zero.

---

### 3.13 `ws_state_for_1271`, the last dead-code item (Amendment 1)

`src/web/commands.rs:1456`, inside the file's `#[cfg(test)] mod tests` (943).

Its only two callers are at 1558 and 1643, and both sit inside tests carrying
`#[cfg(windows)]`: `configured_default_shell_invalid_input_leaves_no_session_state_via_web`
(gate at 1546, `fn` at 1548) and
`configured_default_shell_invalid_payload_leaves_no_session_state_via_web` (gate at 1628,
`fn` at 1630). So the helper is unused in `lib test` off Windows.

| # | Attribute line | Applies to | Ruling |
|---|---|---|---|
| B14 | insert between the doc comment ending at 1455 and `fn` at 1456 | `fn ws_state_for_1271` | `#[cfg(windows)]` |

Bare `windows`, not `target_os = "windows"`, to match the two callers' own spelling in the
same module. The enclosing `mod tests` already supplies the `test` half of the predicate, so
no `all(test, …)` is needed; this is the same reasoning section 3.4 used for
`delete_workgroup_dir_backend`.

**Cascade checked, and this is the one that matters.** Gating a helper can orphan the
imports only it used. Every type `ws_state_for_1271` names is also named by the ungated
sibling `ws_state_for` (1245) or by other ungated tests: `GitWatcher` (also 1181, 1255),
`IdleDetector` (1180, 1254), `PtyManager` (1182, 1256), `HashMap` (1183, 1257, 1509),
`Mutex` (1182-1183, 1256-1257, 1507-1508), `SessionManager` (1179, 1253),
`WsBroadcaster` (1153, 1177, 1251), `AppSettings` (997, 1245 and many more). So B14 orphans
no import in the test module's `use` block at 944-956.

**Side effect on the arc record, folded into section 6 (Amendment 1).** B14 removes the two
`crate::test_support::test_builder()` occurrences at 1457 and 1480 from the non-Windows
build. It does **not** change `src-tauri/module-arcs.txt`, because that file records
non-test references only and never contained a `web::commands -> test_support` arc; see
section 6 (Amendment 1).

**Windows-invariance.** `#[cfg(windows)]` is satisfied on Windows. Zero Windows-compiled
bytes change.

---

### 3.14 `clippy::needless_return`, nine sites, all in code Windows never compiles (Amendment 1)

Every one of these is the same shape, and it is a shape this repository already writes the
lint-free way elsewhere in the same file. A function ends in a run of mutually exclusive
`#[cfg]` blocks. After `cfg` expansion exactly one block survives and becomes the function's
tail expression. Some blocks were written with `return EXPR;` and some without; the ones with
it fire `needless_return` on the platform where they survive, and are invisible on Windows
because Windows strips them.

**Ruling: delete the `return` keyword and the statement's trailing `;`, leaving the
expression as the block's tail. Delete the whole line where the statement is a bare
`return;`.** No `#[allow]`, no restructuring, no change to any predicate.

| # | File and line | Current statement | After |
|---|---|---|---|
| B15 | `src/path_identity.rs:167` | `return self.open_unix_child(...).map_err(...);` | same expression, no `return`, no `;` |
| B16 | `src/path_identity.rs:229` | `return Ok(VerifiedPathIdentity { ... });` | same, no `return`, no `;` |
| B17 | `src/path_identity.rs:253` | `return self.verify_opened_regular_file(path, &file, false);` | same, no `return`, no `;` |
| B18 | `src/path_identity.rs:427` | `return match self.open_unix_child(...) { ... };` | same, no `return`, no `;` |
| B19 | `src/path_identity.rs:469` | `return if result == 0 { Ok(()) } else { Err(...) };` | same, no `return`, no `;` |
| B20 | `src/path_identity.rs:500` | `return if result == 0 { Ok(()) } else { Err(...) };` | same, no `return`, no `;` |
| B21 | `src/path_identity.rs:527` | `return matches!(self.cleanup_unix_tracked_file(...), ...);` | same, no `return`, no `;` |
| B22 | `src/pty/terminal_snapshot.rs:1622` | `return self.cleanup_artifact_unix(expected);` | same, no `return`, no `;` |
| B23 | `src/pty/terminal_snapshot.rs:1743` | `return self.relocate_artifact_unix(expected, path);` | same, no `return`, no `;` |
| B24 | `src/pty/terminal_snapshot.rs:1809` | bare `return;` | delete the whole line |

**B20 is not in the tech lead's list, and it must be fixed anyway.** Line 469 is inside
`#[cfg(target_os = "linux")]` and line 500 is the macOS twin inside
`#[cfg(target_os = "macos")]`, both in `publish_new_file_atomic` (444). Only one of the two
can ever fire on a given platform, so the Linux run reported 469 and the macOS run reported
500. The packet describes the two frontiers as "identical on both", which is true of the
counts but not of this line pair. Fixing only 469 leaves macOS red at 500 and burns an
iteration; both are therefore specified.

**Only the tail `return` goes. Every early `return` stays**, and there are several in the
same functions: `path_identity.rs:98, 101, 104, 216, 227, 525, 540` and
`terminal_snapshot.rs:1635, 1639, 1727, 1771, 1773, 1776, 1783, 1786, 1804, 1818`. Those are
genuine control flow, `needless_return` does not fire on them, and removing one would be a
behaviour change.

**Why this compiles, proved on Windows.** The transformation relies on "an attributed block
that is last after `cfg` stripping is the function's tail expression, and its own tail
expression is the function's value". That is not a hopeful reading: the very same functions
already carry the lint-free half of the pair, and Windows compiles it today.

| Function | Windows-compiled arm that already has no `return` |
|---|---|
| `create_new_file` (160) | `#[cfg(not(unix))]` block at 179-197, tail `options.open(path).map_err(...)` |
| `verify_opened_regular_file` (200) | `#[cfg(not(unix))]` block at 236-240 |
| `verify_regular_file` (243) | `#[cfg(not(unix))]` block at 255-259 |
| `child_is_absent` (424) | `#[cfg(not(unix))]` block at 436-441 |
| `publish_new_file_atomic` (444) | `#[cfg(windows)]` block at 506-509 |
| `remove_regular_file_if_same` (517) | `#[cfg(windows)]` block at 532-535 |
| `cleanup_artifact` (1614) | `#[cfg(not(unix))]` block at 1624-1650, tail is a `match` |
| `relocate_artifact` (1735) | `#[cfg(not(unix))]` block at 1745-1791, tail `Ok(Some(current))` |
| `sweep_artifacts` (1794) | returns `()`, so B24 only needs the statement gone |

So each edited arm is being brought into the shape its own sibling arm already proves
compiles.

**Windows-invariance.** B15 through B24 are all inside `#[cfg(unix)]`,
`#[cfg(target_os = "linux")]` or `#[cfg(target_os = "macos")]` blocks. Windows compiles none
of them. Zero Windows-compiled bytes change.

**Formatting.** `cargo fmt` is not `cfg`-aware and formats every token in the file
regardless of platform, so running it on Windows correctly reflows these edits. Removing
`return` from a multi-line chained expression will change indentation; that reflow is
expected and is inside section 4.1's file list.

---

### 3.15 `clippy::drop_non_drop` at `src/agent_update.rs:434` (Amendment 1)

The only item in the second frontier that touches code Windows **does** compile, so it gets
the most careful treatment.

`job` is `Option<crate::pty::job::JobObject>` (364-373). `JobObject` resolves to
`windows_impl::JobObject` on Windows, which holds a handle and has `impl Drop` at
`src/pty/job.rs:130`, and to `stub_impl::JobObject` elsewhere, a unit struct with **no**
`Drop` (`job.rs:142-156`). So `drop(job)` is meaningful on Windows and a no-op clippy
rejects everywhere else.

**Rejected alternative: give the stub an `impl Drop`.** It is tempting, and it would mirror
section 3.5, where the fix for `new_without_default` was to add to the non-Windows module
the impl its Windows sibling has. It is refused because the stub's own doc comment
(`job.rs:144-148`) states the opposite fact:

> No-op Job Object for non-Windows builds. The Win32 tree-kill primitive does not exist
> here, so PtyManager simply never holds one (`for_child` -> None). `#[allow(dead_code)]`
> because the unit struct is never constructed on this platform.

A `Drop` impl on a type the module says is never constructed is ceremony that can never run,
and it would contradict the module's written contract. Section 3.5 added an impl that a real
caller needs; this would add one nothing can reach.

**Ruling, item B25.** Keep `drop(job)` on Windows byte-for-byte and say off Windows what is
true: there is no job to drop. `src/agent_update.rs`, replacing line 434 inside the `if let`
at 433-435:

```rust
                        if let Some(job) = job.take() {
                            // Windows: dropping the handle closes the job, and
                            // KILL_ON_JOB_CLOSE reaps the lingering tree member.
                            // Off Windows `JobObject::for_child` always returns
                            // `None` (`pty::job::stub_impl`), so this arm is
                            // unreachable and the stub has no `Drop` to call.
                            #[cfg(windows)]
                            drop(job);
                            #[cfg(not(windows))]
                            let _ = job;
                        }
```

- The `if let` and `job.take()` are untouched, so `job` stays `mut`-used on every platform
  and no `unused_mut` appears. Gating the whole `if let` would have broken that.
- `#[cfg]` on a statement is stable and is this repository's idiom, including in this very
  function at 436 and in `path_identity.rs:161, 164, 179`.
- `let _ = job;` binds and discards a type with no `Drop`, so no lint applies:
  `clippy::let_underscore_drop` is a `restriction` lint, allow-by-default, and `-D warnings`
  escalates only warn-by-default lints.
- Line 495's `if let Some(job) = &job` is untouched and still reads the option.

**Windows-invariance.** After `cfg` expansion on Windows the statement is `drop(job);`,
identical to today. Zero Windows-compiled bytes change.

---

### 3.16 `unused variable: fixture` at `acceptance_tests.rs:958` (Amendment 1)

**Ruling: dev-rust's uncommitted edit is ratified exactly as it stands on disk.**

`src/pty/terminal_snapshot/acceptance_tests.rs`, in `consume_host_response` (957):

```rust
    // `fixture` is read only by the non-Unix untracking below; keep the
    // signature stable on Unix rather than splitting the helper.
    #[cfg(unix)]
    let _ = fixture;
    #[cfg(not(unix))]
    fixture.snapshot_state.untrack_artifact(&identity);
```

The parameter's only use was the `#[cfg(not(unix))]` line, so on Unix it was unused. The
`#[cfg(unix)] let _ = …;` form is this repository's established answer to exactly that
situation: `path_identity.rs:166` (`let _ = lock_output_leaf;`), `path_identity.rs:182`
(`#[cfg(not(windows))] let _ = lock_output_leaf;`), `path_identity.rs:412`
(`let _ = final_claim;`) and `path_identity.rs:512` (`let _ = (source, destination);`).

Rejected alternatives: renaming the parameter to `_fixture` would rename it at every caller
and lose the name at the one place it is used; splitting the helper into two `cfg` arms
would duplicate the whole body for one line of difference.

**Windows-invariance.** The added statement is `#[cfg(unix)]`, so Windows compiles nothing
new. The `#[cfg(not(unix))]` line is untouched. Zero Windows-compiled bytes change.

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

- **Bound: three push-and-read iterations.** If the gates are not green after the third, stop and report the remaining diagnostics rather than continuing. *(Amendment 1 replaces this bound; see step 6, amended.)*
- Each iteration fixes only diagnostics reported by that run. Do not pre-emptively gate symbols CI has not flagged. *(Amendment 1 narrows this; see step 6, amended.)*
- Every new fix must follow section 3.0: `cfg` narrowing where the symbol is platform-specific in fact, a real implementation where something is genuinely missing, never `#[allow]`, never `continue-on-error`, never dropping `-D warnings`, never `|| true`. *(Amendment 1: section 3.0a supersedes section 3.0 on the `#[allow]` clause only.)*
- After each iteration, re-run step 4 before pushing again. A Linux or macOS fix that breaks Windows is a regression, not progress.
- If a diagnostic cannot be resolved without an `#[allow]`, stop and report it as an open decision instead of adding one.

---

### Step 4a. Run the source-scanning guard tests on Windows (Amendment 1, mandatory)

**This step exists because its absence is what cost iteration 2.** Steps 2 and 4 run
`cargo check` and `cargo clippy` and never `cargo test`, but the Windows gate
`rust-regression` runs `cargo test --lib --bins --tests`. A guard that reads source text and
asserts an equality over it is invisible to `check` and `clippy` and only fails under `test`.

From `src-tauri`, after every edit and before every push:

```
cargo test --test project_settings_layering --test loops_layering --test instance_gitignore_layering --test claude_watcher_layering --test pty_writer_inventory
```

All five must pass. They are pure source-text scans with no PTY, no network and no
filesystem fixture, so they are cheap; the full `cargo test --lib --bins --tests` is not
mandated here because its acceptance suites are not, and the CI gate remains the authority
on the rest.

The five are the complete set of source-scanning guards under `src-tauri`, established by
searching every file that reads `env!("CARGO_MANIFEST_DIR")`. Two further in-lib scans were
found and checked: `src/session/selection.rs:3946`
(`production_selection_and_lifecycle_sources_have_one_owner`, scans all of `src/` for
lifecycle-ownership violations) and `src/lib.rs:3609`
(`restore_loop_normalizes_archived_roots_before_persisted_session_loop`, scans the
production half of `lib.rs`). Neither pins anything this plan changes, and both run under
`--lib` in CI.

### Step 6, amended (Amendment 1): iteration budget

**The original bound of three is spent: `9c7c77e5` and `6c792b3f` are both on the branch,
and the second is red. The amended budget is two further push-and-read iterations, that is,
at most four on the branch in total.**

- **Iteration 3** lands sections 3.10 through 3.16 in one push, plus dev-rust's two
  uncommitted edits. It is a single push, not one per section.
- **Iteration 4 is reserved for second-order fallout only**, that is, a diagnostic whose
  cause is a `cfg` this plan narrowed (an orphaned import, a newly unused static, a newly
  unused helper). Fix those under section 3.0a and push once.
- **After iteration 4, stop and report regardless of colour.** If what iteration 4 reveals
  is a genuinely new class rather than fallout, that is the third frontier of section 8, and
  it is the coordinator's decision whether it belongs on this branch. Do not open a fifth.

The "fix only what CI reported" rule is narrowed for iteration 3 only: this amendment rules
in advance on symbols CI has not yet flagged, specifically `path_identity.rs:500` (section
3.14, B20) and `WINDOW_SCREENSHOT_AUDITS_FOR_TEST` (section 3.11.2, B3). Both are predicted
consequences of fixes CI did report, and holding them back would spend an iteration proving
something already known. Nothing else may be pre-emptively gated.

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

### 4.1 (Amendment 1) Complete file change matrix for iteration 3

The matrix above describes iterations 1 and 2, which are already committed as `9c7c77e5`
and `6c792b3f`. The matrix below is the **complete and exclusive** set of files iteration 3
may change, measured from `6c792b3f`.

| File | Change | Section | Uncommitted on disk already? |
|---|---|---|---|
| `src-tauri/src/web/event_broadcast.rs` | one call site reverted off `test_support` to an inline Tauri predicate | 3.10 | **yes**, ratified as written |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | `#[cfg(unix)] let _ = fixture;` added | 3.16 | **yes**, ratified as written |
| `src-tauri/src/api/audit.rs` | 10 attributes, B1 through B10 | 3.11.2 | no |
| `src-tauri/src/api/error.rs` | 7 attributes, B11 through B13 | 3.11.3 | no |
| `src-tauri/src/web/commands.rs` | 1 attribute, B14 | 3.13 | no |
| `src-tauri/src/path_identity.rs` | 1 comment + 1 `#[allow]` (3.12); 7 `return` removals, B15 through B21 | 3.12, 3.14 | no |
| `src-tauri/src/pty/terminal_snapshot.rs` | 3 `return` removals, B22 through B24 | 3.14 | no |
| `src-tauri/src/agent_update.rs` | 1 statement replaced by a 2-arm `cfg` split plus comment, B25 | 3.15 | no |
| `plans/1154-nonwindows-rust-gate-failures.md` | this amendment, `git add -f` | this file | no |

**No other file may change in iteration 3.** In particular
`src-tauri/tests/project_settings_layering.rs` is **not** edited: no allowlist row is added,
no table is touched, and section 4.3 of `plans/1265-extract-project-settings-from-scc.md` is
not rewritten. `src-tauri/src/test_support.rs`, `src-tauri/src/lib.rs`,
`src-tauri/src/pty/job.rs`, `.github/workflows/pr-regression-gates.yml`,
`src-tauri/module-arcs.txt`, `crates/**`, `src/**`, `Cargo.toml`, `Cargo.lock` and
`package.json` are all untouched.

`module-arcs.txt` appears in the matrix above but not here, because section 6 (Amendment 1)
establishes that iteration 3 changes it by zero lines. `npm run record:arcs` is still run,
to prove that.

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
13. Section 6's dependency-cycle criterion holds. *(Replaced by section 6 (Amendment 1); criterion A9 below.)*

### 5 (Amendment 1) Acceptance criteria for iteration 3

Criteria 1 through 4 and 7 through 12 above still hold as written. Criteria 5, 6 and 13 are
amended here, and A1 through A11 are added. All of them must be true.

**A1.** `cargo test --test project_settings_layering` passes, all three of its tests, with
`ALLOWED_EMITTER_CRATE_REFERENCES`, `ALLOWED_EMITTER_WEB_REFERENCES` and
`ALLOWED_EMITTER_SUPER_REFERENCES` each still holding exactly one row, unchanged from
`6c792b3f`. Reinforcing that mechanically:
`grep -n "test_support" src-tauri/src/web/event_broadcast.rs` returns nothing outside the
explanatory comment, and `git diff origin/main -- src-tauri/src/web/event_broadcast.rs`
shows the file's set of `crate::`-anchored and `web::`-anchored references identical to
`origin/main`.

**A2.** `git diff origin/main -- src-tauri/tests/project_settings_layering.rs` is empty. No
allowlist row was added, removed or reordered by this branch.

**A3.** The Tauri platform predicate is spelled identically in both places that carry it.
Each of `src-tauri/src/test_support.rs` and `src-tauri/src/web/event_broadcast.rs` contains
exactly one line whose trimmed text is

```
#[cfg(any(windows, target_os = "linux"))]
```

and exactly one whose trimmed text is

```
#[cfg(not(any(windows, target_os = "linux")))]
```

and no other file under `src-tauri/src` contains either. Prose occurrences of the predicate
inside doc comments do not count and are expected in both files; only attribute lines are
compared. Any drift between the two files is a defect, because both exist to mirror
`tauri-2.10.3/src/app.rs:1503`.

**A4.** `grep -rn "\.any_thread()" src-tauri/` returns exactly **four** lines: one in
`src-tauri/src/test_support.rs`, one in `src-tauri/src/web/event_broadcast.rs`, and the two
pre-existing lines at `src-tauri/tests/pty_lifecycle_regression.rs:285,313`. This supersedes
criterion 6's count of three.

**A5.** Every item B1 through B25 is present with the exact predicate or edit given in
sections 3.10 through 3.16, and **no symbol was deleted or renamed** by this amendment.

**A6.** On the pull request, `rust-regression-linux` and `rust-regression-macos` both
conclude **success**, which is to say `cargo clippy --all-targets -- -D warnings` reports
**zero** diagnostics on both, for every target it reaches. This is stated as zero and never
as a count of fixed items: the packet's totals of 17 in `lib` and 19 in `lib test` could not
be reconciled item by item with its own enumeration from a Windows host, so the count is not
a criterion. Section 8 records what "every target it reaches" does and does not cover.

**A7.** On the pull request, `rust-regression` (Windows) concludes success, including its
`cargo test --lib --bins --tests` step, and `frontend-regression`, `test-debt`,
`lockfile-drift`, `windows-release-cli-smoke`, `validate-branch-name` and
`terminal-snapshot-portable` all still conclude success.

**A8.** Locally on Windows, before the push: step 2's two commands are clean, and step 4a's
five guard tests all pass.

**A9.** Section 6 (Amendment 1)'s criterion holds: after `npm run record:arcs`,
`git diff -- src-tauri/module-arcs.txt` is **empty**, and
`git diff origin/main -- src-tauri/module-arcs.txt` remains exactly one added line,
`agentscommander_lib::pty::local_backend -> agentscommander_lib::test_support`, with zero
removed and zero other added lines.

**A10, amending criterion 5.** The branch adds **exactly one** `#[allow(...)]` attribute,
the `#[allow(clippy::enum_variant_names)]` of section 3.12. Mechanically:
`git diff origin/main -- src-tauri/src src-tauri/tests | grep '^+' | grep '#\[allow('`
returns exactly one line. The `#[allow(dead_code)]` half of criterion 5 is unchanged and
still returns zero added lines.

**A11.** The committed file set for iteration 3 equals section 4.1 (Amendment 1) exactly,
with this plan force-added.

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

## 6 (Amendment 1). Dependency-cycle gate, re-run over the amended design

Applied per the `verify-no-dependency-cycles` skill, with one property added that the first
pass did not model. **Verdict: PASS, no new cycle, no SCC growth, no cross-boundary arc, and
no pinned reference set moved.**

### 6.1 The first pass's arc claim was wrong, and the measurement says so

Section 6 above predicted nine added arcs and wrote an acceptance criterion around them.
Measured on the branch at `6c792b3f`:

```
git diff origin/main --stat -- src-tauri/module-arcs.txt
  src-tauri/module-arcs.txt | 1 +
```

**One** line was added, not nine:

```
agentscommander_lib::pty::local_backend -> agentscommander_lib::test_support
```

The record is 1008 lines and contains no other `test_support` arc. The classification in
section 6 was sound; the arc **count** was not, and the criterion built on it could never
have been satisfied. It is replaced below.

Why one and not nine. `scripts/02-module-arc-record.mjs` refuses any graph not emitted with
`includeTests: false` (line 280), so the record is defined over non-test references only,
and every call site section 3.7 rewrote sits inside test code. The single site that was
recorded, `src/pty/local_backend.rs:3852`, is inside
`#[cfg(all(test, windows))] mod adapter_spawn_sync_tests` (3841). The most likely reason it
was counted is that the upstream detector's test exclusion recognises `#[cfg(test)]` and not
the compound `#[cfg(all(test, windows))]` form, but that is a **hypothesis about a tool this
plan does not own**, and it is written here as one. It does not need to be true for the
criterion below to be correct, because the criterion is stated as "no change", not as a
prediction of which sites are counted.

Note also that `agentscommander_lib -> agentscommander_lib::test_support` never appeared:
the record carries reference arcs, not `mod` declarations.

### 6.2 Arcs added and removed by Amendment 1: none

- **Section 3.10** removes one `crate::test_support` reference from
  `src/web/event_broadcast.rs`. That reference was inside `#[cfg(test)] mod tests` and was
  never in the record, so removing it removes no arc. It adds no reference of any kind: the
  replacement names only `tauri::Builder`, an external crate.
- **Section 3.13** removes two `crate::test_support` references from `src/web/commands.rs`
  on non-Windows builds only, both inside `#[cfg(test)] mod tests`, and neither is in the
  record.
- **Sections 3.11, 3.12, 3.14, 3.15 and 3.16** add and change `cfg` attributes, one
  `#[allow]`, and delete `return` keywords. Not one of them introduces or removes a
  module-to-module path. `src/agent_update.rs` already names `crate::pty::job` at 364 and
  367 and continues to.

Therefore **zero arcs are added and zero removed**, `cyclicSccs` is unchanged, every SCC
member set is identical, and no arc crosses a previously-clean SCC boundary. The
`test_support` node keeps intra-crate out-degree zero, so the section 6 classification
remains valid for the one arc that does exist.

### 6.3 The property this gate was missing, and now checks

The emitter guard is **not** a reachability property and no SCC computation would have
caught it. It is an **equality over a pinned reference set**: a named module may name a
named set of children under a named anchor, and both growth and shrinkage fail. Section 6's
out-degree-zero argument answered "can this arc close a cycle" correctly and answered "may
this module name that at all" not at all.

**New mandatory step in this gate, from now on.** For every file a plan edits, check whether
it appears in a pinned reference table, and if it does, whether the edit moves that file's
observed set under any anchor. The complete inventory of such tables under `src-tauri`,
established for this amendment:

| Guard file | Pinned file | Anchors pinned |
|---|---|---|
| `tests/project_settings_layering.rs` | `src/commands/project_settings.rs` | `web::` (204), alias check |
| `tests/project_settings_layering.rs` | `src/web/event_broadcast.rs` | `crate::` (215), `web::` (228), `super::` (250), alias check |
| `tests/loops_layering.rs` | `src/loops/delivery.rs` and the rest of `src/loops/` | `commands::` (89) |
| `tests/instance_gitignore_layering.rs` | `src/config/instance_gitignore.rs` | `crate::` (503), `super::` (539), `self::` (578) |
| `tests/instance_gitignore_layering.rs` | `src/config/mod.rs` | `crate::` (555), `super::` (565), `self::` (591) |
| `tests/claude_watcher_layering.rs` | `src/telegram/claude_watcher.rs`, `src/telegram/output.rs` | module-level, via the authoritative topology |
| `tests/pty_writer_inventory.rs` | all of `src/` | the `write_with_permit(`, `backend.write(`, `route_guard.write(` and `for_route_guard` capability sets |
| `src/session/selection.rs:3946` | all of `src/` | lifecycle-ownership violations |
| `src/lib.rs:3609` | `src/lib.rs` production half | restore-loop ordering |

Applying it to Amendment 1's eight edited files:

- `src/web/event_broadcast.rs` is pinned, and section 3.10 exists precisely to return its
  three sets to their `origin/main` values. Checked.
- `src/web/commands.rs` is named by `tests/pty_writer_inventory.rs` (72, 122), which pins
  that the file contains `write_with_permit(` and `acquire_input_writer(`. B14 adds one
  `#[cfg(windows)]` line and touches neither needle. Checked.
- `src/api/audit.rs`, `src/api/error.rs`, `src/path_identity.rs`,
  `src/pty/terminal_snapshot.rs`, `src/agent_update.rs` and
  `src/pty/terminal_snapshot/acceptance_tests.rs` appear in no table. The two whole-tree
  scans (`selection.rs:3946`, `pty_writer_inventory.rs` step 2) read them, but only for
  needles none of these edits introduce: no `SessionStatus` assignment, no `app.emit` of a
  lifecycle event, no `write_with_permit(`, no `backend.write(`, no `route_guard.write(`, no
  `for_route_guard`. Checked.
- Step 4a makes this check executable rather than a promise.

### 6.4 Role and layering hygiene

Unchanged from section 6 and re-verified. `test_support` remains a new top-level leaf
returning a `tauri::Builder`; no pre-existing lower-layer module gains a UI-transport
dependency it did not already have. Section 3.10 moves one test's builder construction back
into `web::event_broadcast`, which already constructed a `tauri::Builder` at that line before
#1154, so no layer boundary moves. No pure predicate is pushed below a transport boundary
and no transport-taking function is pushed downward.

### 6.5 Step-N detector acceptance criterion (Amendment 1)

**This replaces the nine-line criterion of section 6.** After running `npm run record:arcs`
from the repository root at the end of iteration 3:

1. `git diff -- src-tauri/module-arcs.txt` is **empty**. Iteration 3 changes the record by
   zero lines.
2. `git diff origin/main -- src-tauri/module-arcs.txt` shows **exactly one** added line,
   `agentscommander_lib::pty::local_backend -> agentscommander_lib::test_support`, with zero
   removed lines and zero other added lines.
3. Step 4a's five guard tests pass, which is the executable form of the property section 6.3
   adds to this gate.

Any other delta means an unintended module reference was introduced, or the recorder's
inputs changed. Stop and report rather than accepting it.

---

## 7. Open items for the coordinator

Listed for decision. None of them blocks implementation of sections 3 through 6.

1. Whether PR #1156 may absorb #1132 through #1136, per section 1. If the answer is no, sections 3.1 through 3.6 lift out onto five per-issue branches unchanged, and this branch keeps only step 1, section 3.7 and the workflow merge.
2. Filing `B14-SELECTION-TEST-CFG` and `B15-TAURI-ANY-THREAD-MACOS` as children of #1113 for sections 3.6 and 3.7.
3. Updating epic #1113's checklist and the affected child issue bodies to record where the work actually landed.
4. Whether `cargo test` should ever run on macOS, which would require revisiting section 3.7's accepted run-time consequence.
5. *(Amendment 1)* PR #1156 now also absorbs the #1271 window-screenshot audit and error
   surface (sections 3.11.2 and 3.11.3), which is not one of #1132 through #1136. Add it to
   whatever record open item 1 produces, or the same gap will be re-found later.
6. *(Amendment 1)* Section 8's frontier-3 question, if it materialises: whether it lands here
   or in a follow-up PR.

---

## 8. Is a third frontier likely? (Amendment 1)

**Yes, and it is stronger than likely: there is a surface both non-Windows jobs have never
once measured, and it is guaranteed to be unmeasured rather than merely suspected. What is
unknown is only whether it is dirty, not whether it is unexamined.**

### 8.1 Why frontiers exist at all here

Each job runs, from `src-tauri`:

```
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
```

`--all-targets` selects `lib`, `lib test`, the `main` bin, and every integration-test crate
under `src-tauri/tests/`. `-D warnings` turns a lint into a compilation error, so a dirty
`lib` unit **fails**, produces no artifact, and every unit that links the library is never
scheduled. `lib test` is a separate compilation of the same sources and does not link the
library, which is exactly why the packet could report `lib` and `lib test` together and
nothing else.

That is the mechanism that manufactures frontiers, and it is why each one looked like a
surprise:

| Frontier | Surface | Status |
|---|---|---|
| 1 | macOS `cargo check`, plus `lib`/`lib test` dead code on Linux | cleared by `9c7c77e5` and `6c792b3f` |
| 2 | `lib` and `lib test` clippy on Linux and macOS | this amendment |
| 3 | the `main` bin and the integration-test crates, clippy, on Linux and macOS | **never once compiled by clippy on either platform** |

### 8.2 What frontier 3 contains, measured

`src-tauri/tests/` holds **25** integration-test crates totalling roughly 900 KB of Rust,
plus `src-tauri/src/main.rs`. One of the 25, `pty_lifecycle_regression.rs`, is
`#![cfg(target_os = "windows")]` and compiles to nothing off Windows, leaving **24 live
crates**. Several are large: `claude_watcher_layering.rs` at 108 KB,
`instance_gitignore_layering.rs` at 101 KB, `project_settings_layering.rs` at 65 KB,
`cli_workgroup_team.rs` at 61 KB, `pty_powershell_managed_native.rs` at 58 KB,
`wake_consumption_measure.rs` at 57 KB.

None of it has been linted on Linux or macOS by any run in this repository's history,
because clippy has never got past `lib`.

### 8.3 What bounds it, and why this is not open-ended

Two facts bound frontier 3 tightly, and both are already measured.

1. **It cannot contain a compile error.** `cargo check --all-targets` is reported green on
   both Linux and macOS at `6c792b3f`, and `check` does schedule every target because it
   emits no `-D warnings`. So every one of the 24 crates and the bin already **compiles** on
   both platforms. Whatever frontier 3 holds is clippy lints only: no `E0599`, no missing
   `cfg`, nothing of the shape that made macOS fail at frontier 1.
2. **There is no frontier 4.** `--all-targets` is exhaustive over this package, and the two
   jobs deliberately omit `--workspace` (section 3.9), so `crates/session-bridge` and
   `crates/terminal-snapshot-renderer` are out of scope here and are already covered on
   Linux and macOS by `terminal-snapshot-portable`. Once frontier 3 is clean, the authored
   commands have nothing left to reach.

### 8.4 What it will most likely look like

Frontier 2's composition is the best available predictor, and the integration-test crates
are written in the same style by the same team:

- `dead_code` for Windows-only helpers in crates that carry no crate-level platform gate.
  Two are already known to be in this class: section 2.2 recorded that
  `pty_powershell_managed_native.rs` and `wake_consumption_measure.rs` carry no crate-level
  `cfg` and contain `cfg(windows)` regions, which is the exact shape that produced R4
  through R9.
- `needless_return` and `unused variable` in `#[cfg(unix)]` arms, the shape of sections 3.14
  and 3.16.
- The occasional real lint in shared code, the shape of section 3.15.

All three classes are resolved by rules this plan already contains. That is the encouraging
half of the assessment: frontier 3 should need judgement, not new design.

### 8.5 Recommendation on the container

**Keep it on PR #1156 through iteration 4, then reassess. Do not split now.**

For keeping it:

- The two gates cannot go green in pieces. A partial fix leaves `rust-regression-linux` and
  `rust-regression-macos` red, which is a worse review artifact and a worse merge signal
  than one broad PR.
- The branch already carries the 342-commit merge and is `mergeable: true`. Splitting means
  redoing that merge on every new branch, for no gate benefit.
- #1154's own acceptance criterion is "these two jobs pass". That is a single, indivisible
  outcome.

For splitting later, if frontier 3 turns out to be large:

- The natural cut is exactly the frontier boundary. `src-tauri/tests/**` and
  `src-tauri/src/main.rs` are a disjoint file set from everything this PR has touched so
  far, so a follow-up PR would have a zero-overlap diff and could be reviewed on its own.
- If the coordinator wants #1156 merged before frontier 3 is cleared, the lever is a
  repository setting and not a workflow edit: make `rust-regression-linux` and
  `rust-regression-macos` non-required checks until they are green, then require them. That
  keeps criterion 3 intact, since it forbids `continue-on-error`, `|| true` and relaxing
  `-D warnings` inside the workflow, and says nothing about branch-protection policy. **This
  is a coordinator decision, not a plan change, and this plan does not take it.**

### 8.6 The honest bottom line

The first pass said in writing that its inventory was "the complete inventory of *known*
defects ... not a guarantee that no *unknown* defect exists". That framing was right and it
is still right, but it can now be made much sharper, and sharper is what was asked for:

> There is exactly one unmeasured surface left, it is enumerable at 24 crates plus one bin,
> it is proved to compile on both platforms, it can contain lints and nothing worse, and
> after it there is no further surface under the authored commands.

If the team is willing to spend one more loop after iteration 4, that loop terminates the
problem. If it is not, section 8.5's second lever is the way to merge without pretending the
surface is clean.
