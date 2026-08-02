# Implementation Plan: #1208 Guard test-only Tauri `any_thread` calls on macOS

Status: READY_FOR_IMPLEMENTATION

Delivery path: Lite

Baseline commit: `a784474c64f0aeca77e1eddb50f127089836e0cf`

Implementation branch: `fix/1208-macos-test-builder`

Issue: https://github.com/mblua/AgentsCommander/issues/1208

## 1. Issue and objective

PR #1156 adds a GitHub `macos-latest` Rust regression job whose first Rust step is `cargo check --all-targets`. That step compiles the library test target and currently fails with exactly eight `E0599` errors. Each error comes from macOS-visible `#[cfg(test)]` code calling `tauri::Builder::any_thread()`, a method that Tauri 2.10.3 does not expose on macOS.

The objective is to introduce one crate-local test-only builder policy and route exactly those eight library-test builders through it. The policy must:

- return `tauri::Builder::default().any_thread()` on Windows and Linux;
- return `tauri::Builder::default()` on macOS and every target on which Tauri does not expose `any_thread`;
- retain the default Wry runtime and every existing `.manage(...)`, `.build(...)`, and error expectation at the call sites;
- make the macOS all-target check and clippy steps reachable without changing production construction or making Wry-backed tests execute on macOS worker threads.

No implementation decision is left open in this plan.

## 2. Evidence and identified cause

### 2.1 Baseline and CI evidence

- The branch, local `main`, and `origin/main` were verified clean and byte-identical at `a784474c64f0aeca77e1eddb50f127089836e0cf`.
- The issue records GitHub Actions run `30216570010`, macOS job `89831713473`, and step exit code 101. The `cargo check --all-targets` step fails before clippy can run.
- The verified Step 1 report is `.ac/wg-1-dev-v4-team/messaging/20260802-022548-wg1-dev-rust-to-wg1-tech-lead-macos-ci-report.md`. It independently reconstructs the eight diagnostics from the immutable PR-head and current-main sources, the locked dependency, and live GitHub job/check metadata.
- At this baseline, `.github/workflows/pr-regression-gates.yml` still contains only the existing Windows Rust job. PR #1156 is open and adds the Linux and macOS jobs without changing application source. Final macOS evidence for #1208 must therefore come from a GitHub revision containing both the #1208 source fix and the unmodified #1156 workflow. This issue must not copy or edit that workflow.

### 2.2 Complete `any_thread` inventory

An exact-text audit at the baseline finds twelve calls:

| # | File and baseline line | Enclosing symbol | Classification |
| ---: | --- | --- | --- |
| 1 | `src-tauri/src/commands/ac_discovery.rs:3113` | `tests::archive_command_app` | in scope |
| 2 | `src-tauri/src/commands/config.rs:2579` | `tests::api_server_command_test_app` | in scope |
| 3 | `src-tauri/src/lib.rs:3245` | `tests::web_and_api_server_handles_can_be_managed_together` | in scope |
| 4 | `src-tauri/src/loops/delivery.rs:564` | `tests::make_inject_test_app` | in scope |
| 5 | `src-tauri/src/web/commands.rs:1139` | `tests::broadcast_all_r_sends_to_managed_websocket_broadcaster` | in scope |
| 6 | `src-tauri/src/web/commands.rs:1161` | `tests::broadcast_all_sends_to_explicit_websocket_broadcaster` | in scope |
| 7 | `src-tauri/src/web/commands.rs:1180` | `tests::update_project_groups_web_dispatch_broadcasts_saved_config` | in scope |
| 8 | `src-tauri/src/web/commands.rs:1254` | `tests::ws_state_for` | in scope |
| 9 | `src-tauri/tests/wake_consumption_measure.rs:209` | `make_ctx`, Git watcher app | out of scope |
| 10 | `src-tauri/tests/wake_consumption_measure.rs:233` | `make_ctx`, harness app | out of scope |
| 11 | `src-tauri/tests/pty_lifecycle_regression.rs:257` | `make_test_app`, Git watcher app | out of scope |
| 12 | `src-tauri/tests/pty_lifecycle_regression.rs:284` | `make_test_app`, lifecycle app | out of scope |

All eight in-scope sites are inside inline `#[cfg(test)] mod tests` modules in the library crate. The other four are in two integration-test crates protected at the crate root by `#![cfg(target_os = "windows")]`. They are never macOS compilation targets and must remain byte-unchanged.

### 2.3 Tauri target predicate is the cause

`Cargo.lock` pins `tauri 2.10.3`. In the exact upstream `tauri-v2.10.3` source:

- `Builder::runtime_any_thread` exists only under `#[cfg(any(windows, target_os = "linux"))]`;
- its constructor initialization uses the same predicate;
- `Builder::any_thread` uses the same predicate;
- the method documentation says macOS applications must execute on the main thread, so the method is not exposed there.

Primary source: https://github.com/tauri-apps/tauri/blob/tauri-v2.10.3/crates/tauri/src/app.rs#L1498-L1508

This is intentional target availability, not a missing Cargo feature, Rust-version issue, or warning-policy issue. A source call that is visible on macOS cannot compile.

### 2.4 The affected tests require the default Wry runtime

The eight sites currently use `tauri::Builder::default()` with `tauri::test::mock_context(...)`; they do not use `tauri::test::mock_builder()`.

Two concrete type constraints rule out a narrow MockRuntime substitution:

- `GitWatcher::new` accepts a non-generic `tauri::AppHandle`. It is reached by `archive_command_app`, `api_server_command_test_app`, `make_inject_test_app`, `update_project_groups_web_dispatch_broadcasts_saved_config`, and `ws_state_for`.
- `WsState.app_handle` is a non-generic `tauri::AppHandle`. It is built by the web-command test helpers.

Changing those production-shaped types to accept `AppHandle<MockRuntime>` would introduce runtime generics or production type changes and would require Full planning. It is not needed for the compile-only macOS objective.

The repository has 45 existing `mock_builder()` calls, including a nearby one in `commands/config.rs`. Those are valid analogues only where the test state already accepts `MockRuntime`; they are not a drop-in replacement for these eight Wry sites.

### 2.5 Codebase Memory and fallback disclosure

The complete macOS full gate succeeded on indexed `main` at the baseline SHA with project:

`Users-mblua-Documents-GitHub-AgentsCommander_iac-.ac-wg-1-dev-v4-team-repo-AgentsCommander`

Every graph query was immediately preceded by a zero-change freshness guard.

Supporting graph work:

- a function search for the affected helper/test names returned exactly the eight in-scope symbols and their five files;
- snippets verified each builder chain and its enclosing `#[cfg(test)]` context;
- caller traces mapped nine callers of `archive_command_app`, one caller each for `api_server_command_test_app` and `make_inject_test_app`, and six callers of `ws_state_for`;
- a runtime-type search plus snippets verified the non-generic `GitWatcher::new(..., AppHandle)` and `WsState.app_handle: tauri::AppHandle` constraints;
- the graph had no useful exact `any_thread`, test-builder, or `mock_builder` symbol result.

Permitted targeted fallbacks filled those graph gaps:

- `rg -n '\.any_thread\(' src-tauri` established the complete twelve-call inventory;
- `rg -n 'mock_builder\(' src-tauri/src` established the 45 existing MockRuntime uses;
- direct reads covered each affected test module, the nearby mock-builder analogue, both excluded integration-test crate predicates and call contexts, `Cargo.toml`, `Cargo.lock`, and the current workflow commands;
- upstream Tauri 2.10.3 source established the method predicate, default Wry feature, public `tauri::Wry` alias, and `Builder<Runtime>` shape.

All source analysis and technical decisions were completed on indexed `main`. The repository was then returned to `fix/1208-macos-test-builder` at the identical SHA before this plan was written.

## 3. Scope

### 3.1 In scope

Only these application-source changes are allowed:

1. Add one crate-root `#[cfg(test)]` builder policy in `src-tauri/src/lib.rs`.
2. Replace the eight listed `tauri::Builder::default().any_thread()` chains with `crate::test_app_builder()`.
3. Preserve the remaining builder chain at every site.
4. Run formatting, exact-target audits, compile/lint gates, and the platform evidence matrix in Section 9.

The plan artifact itself is `plans/1208-macos-test-builder.md`.

### 3.2 Out of scope

- No edit to `.github/workflows/pr-regression-gates.yml`, PR #1156, or its plan.
- No dependency, lockfile, Cargo feature, Rust toolchain, or Tauri version change.
- No production `tauri::Builder`, application startup, runtime state, IPC command, event, or serialized type change.
- No runtime-generic `AppHandle`, `GitWatcher`, `WsState`, or `PtyManager` refactor.
- No conversion of the eight sites to `tauri::test::mock_builder()` or `MockRuntime`.
- No edit to `src-tauri/tests/wake_consumption_measure.rs` or `src-tauri/tests/pty_lifecycle_regression.rs`; their four Windows-only calls stay as written.
- No test deletion, ignore, filter, new target gate, allow attribute, warning-policy relaxation, or narrowing of `--all-targets`.
- No attempt to execute Wry-backed tests on macOS test-worker threads or to add a macOS main-thread test harness.
- No unrelated Linux diagnostic or downstream macOS clippy repair.
- No unrelated refactor, import cleanup, comment rewrite, or formatting churn.

## 4. Decided solution

### 4.1 Add one crate-local policy

In `src-tauri/src/lib.rs`, immediately after the crate imports and before `shutdown_persistence_allowed`, add exactly one test-only helper with this behavior and shape:

```rust
#[cfg(test)]
pub(crate) fn test_app_builder() -> tauri::Builder<tauri::Wry> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        tauri::Builder::default().any_thread()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        tauri::Builder::default()
    }
}
```

The return type is explicitly `tauri::Builder<tauri::Wry>` because upstream `Builder<R>` has no default generic parameter in its type declaration. The repository enables Tauri's default features, including Wry, and Tauri publicly exports `tauri::Wry`.

The mutually exclusive tail-expression blocks are deliberate:

- the `any_thread` expression is compiled only under Tauri's exact availability predicate;
- the complementary branch retains default semantics everywhere else, including macOS;
- there is no temporary binding followed immediately by a return, so `clippy::let_and_return` cannot become a `-D warnings` failure;
- no lint allow is needed.

Do not replace this with duplicated per-site cfg blocks, a macro, a new module, a generic helper, or a MockRuntime helper.

### 4.2 Route exactly eight sites through the helper

At each symbol below, replace only the initial:

```rust
tauri::Builder::default()
    .any_thread()
```

with:

```rust
crate::test_app_builder()
```

Keep the rest of the chain byte-for-byte except for rustfmt-required indentation.

| File | Symbol | Required edit |
| --- | --- | --- |
| `src-tauri/src/commands/ac_discovery.rs` | `tests::archive_command_app` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/commands/config.rs` | `tests::api_server_command_test_app` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/lib.rs` | `tests::web_and_api_server_handles_can_be_managed_together` | start `_app` from `crate::test_app_builder()` |
| `src-tauri/src/loops/delivery.rs` | `tests::make_inject_test_app` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/web/commands.rs` | `tests::broadcast_all_r_sends_to_managed_websocket_broadcaster` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/web/commands.rs` | `tests::broadcast_all_sends_to_explicit_websocket_broadcaster` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/web/commands.rs` | `tests::update_project_groups_web_dispatch_broadcasts_saved_config` | start `app` from `crate::test_app_builder()` |
| `src-tauri/src/web/commands.rs` | `tests::ws_state_for` | start `app` from `crate::test_app_builder()` |

Use the fully qualified `crate::test_app_builder()` path at all eight sites. Do not add imports.

### 4.3 Tests added by this issue

Add no new standalone test function. The policy is a compile-time target selection over a private upstream builder flag:

- macOS `cargo check --all-targets` and clippy prove the unavailable method is not referenced;
- Windows execution of the existing library tests proves their worker-thread Wry construction still uses the policy successfully;
- Linux compile/clippy plus exact source audit prove the Linux branch selects `any_thread`;
- the upstream flag is private, so a direct value assertion would require an unjustified abstraction or dependency.

The existing tests and CI matrix are the correct regression surface.

## 5. Required behavior, edge cases, and failure behavior

### 5.1 Target behavior

| Build context | Required policy result |
| --- | --- |
| Production on every target | helper and all eight call sites are absent because they are under `#[cfg(test)]`; production construction is unchanged |
| Library tests on Windows | `Builder::default().any_thread()`; existing tests may build Wry apps from test-worker threads |
| Library tests on Linux | `Builder::default().any_thread()`; current behavior is unchanged |
| Library tests on macOS | `Builder::default()`; all-target check/clippy compile without naming the unavailable method |
| Library tests on another unsupported target | `Builder::default()`; the complement tracks method availability safely |
| Two Windows-only integration-test crates | their four direct `.any_thread()` calls remain unchanged and protected by crate-level Windows cfg |

### 5.2 Preserved call-site behavior

- Preserve every managed state value and its order.
- Preserve `tauri::test::mock_context(tauri::test::noop_assets())`.
- Preserve every `.expect(...)` message and return type.
- Preserve every caller and existing test annotation.
- Preserve default Wry `tauri::App` and `tauri::AppHandle` types.
- Preserve the `.build(...)` failure contract. A builder/context error still fails the existing test at the same `.expect(...)`; the helper must not catch, translate, or suppress it.

### 5.3 macOS execution boundary

This change makes the library test target compile on macOS. It does not make Wry applications safe to create from Rust's ordinary macOS test-worker threads. The #1156 macOS job intentionally runs check and clippy, not `cargo test`.

If implementation or review discovers that acceptance requires actual macOS execution, a MockRuntime conversion, runtime-generic production state, or a main-thread harness, stop and reclassify to Full. Do not expand the Lite implementation.

### 5.4 Downstream diagnostics

The current macOS job cannot reach clippy. After the fix, clippy may expose a separate pre-existing diagnostic. Do not repair unrelated findings in #1208. Report the exact diagnostic to its owner and leave #1208 acceptance pending until an authoritative macOS run can show both check and clippy green.

Linux already has unrelated diagnostics under #1113. The #1208 criterion is no new builder-related Linux regression, not repair of that inventory.

## 6. Exact affected files and symbols

The implementation source diff must contain exactly these five files:

| File | Symbols |
| --- | --- |
| `src-tauri/src/lib.rs` | new crate-root `test_app_builder`; `tests::web_and_api_server_handles_can_be_managed_together` |
| `src-tauri/src/commands/ac_discovery.rs` | `tests::archive_command_app` |
| `src-tauri/src/commands/config.rs` | `tests::api_server_command_test_app` |
| `src-tauri/src/loops/delivery.rs` | `tests::make_inject_test_app` |
| `src-tauri/src/web/commands.rs` | `tests::broadcast_all_r_sends_to_managed_websocket_broadcaster`; `tests::broadcast_all_sends_to_explicit_websocket_broadcaster`; `tests::update_project_groups_web_dispatch_broadcasts_saved_config`; `tests::ws_state_for` |

The only additional changed path allowed in the complete branch range is this plan artifact.

## 7. Compatibility, performance, and security impact

- Windows and Linux test behavior is preserved because both still set Tauri's `runtime_any_thread` policy through the same public method.
- macOS retains Tauri's default main-thread semantics. The fix removes an invalid compile-time method reference rather than bypassing the platform contract.
- Production binaries, startup, window behavior, PTY behavior, IPC, persistence, configuration, and serialized data are unchanged.
- No dependency, feature, lockfile, toolchain, or workflow changes are needed.
- No new runtime allocation, branch, or performance cost exists in production. Test builds select one expression at compile time.
- No credential, filesystem authority, network boundary, or user input path changes.
- Security impact is positive and narrow: the code stops requesting a thread-policy bypass on a platform where upstream intentionally forbids it.

## 8. Ordered implementation instructions

### MVP

1. Confirm `HEAD` descends from baseline `a784474c64f0aeca77e1eddb50f127089836e0cf` and the working tree contains no unrelated change.
2. In `src-tauri/src/lib.rs`, add the exact `test_app_builder` helper from Section 4.1 after the crate imports.
3. Replace the single in-scope builder in `lib.rs::tests::web_and_api_server_handles_can_be_managed_together`.
4. Replace the helpers in `commands/ac_discovery.rs`, `commands/config.rs`, and `loops/delivery.rs`.
5. Replace all four in-scope builders in `web/commands.rs`.

### Full features

6. Run the exact occurrence audits in Section 9.1. Correct only a missed in-scope substitution. Do not change the four Windows-only integration calls.
7. Confirm all existing `.manage(...)`, `.build(...)`, `.expect(...)`, helper return types, and test annotations remain unchanged.

### Polish

8. Build frontend assets so Rust compiles the same embedded-assets cfg set as CI.
9. Run rustfmt check, all-target check, all-target clippy with warnings denied, and diff hygiene.
10. Obtain the GitHub platform evidence in Section 9.3 on the exact reviewed source revision.

### Extras

None.

## 9. Tests and verification

### 9.1 Exact source and scope audits

Run from the repository root after implementation:

```bash
rg -n '\.any_thread\(' src-tauri
rg -n 'test_app_builder\(' src-tauri/src
git diff --check
git diff --name-only a784474c64f0aeca77e1eddb50f127089836e0cf -- src-tauri/src
git diff --exit-code a784474c64f0aeca77e1eddb50f127089836e0cf -- src-tauri/tests/wake_consumption_measure.rs src-tauri/tests/pty_lifecycle_regression.rs
```

Required results:

- `.any_thread()` has exactly five remaining occurrences: one inside the guarded `test_app_builder` branch and the four unchanged integration-test occurrences.
- `test_app_builder(` has exactly nine occurrences: one definition and eight calls.
- No `.any_thread()` remains in the four non-`lib.rs` application files.
- The `src-tauri/src` changed-path list is exactly the five files in Section 6.
- The integration-test diff command exits 0 with no output.
- `git diff --check` exits 0.

Inspect the helper occurrence and confirm its immediately enclosing expression is guarded by the exact `#[cfg(any(windows, target_os = "linux"))]` predicate. Inspect the four integration occurrences and confirm each file still has its crate-level `#![cfg(target_os = "windows")]`.

### 9.2 Local or platform-capable commands

Run from the repository root:

```bash
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

On Windows, also run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib --bins --tests
```

Do not add a macOS `cargo test` requirement. This issue covers compilation and lint reachability, not Wry execution on macOS test threads.

### 9.3 Required GitHub evidence

Local cross-compilation is not a substitute for the following evidence.

#### macOS, mandatory for final acceptance

Use a GitHub `macos-latest` job on a revision containing the reviewed #1208 source bytes and the unmodified #1156 workflow. The job must:

1. complete checkout, Node setup, `npm ci`, and `npm run build`;
2. pass `cargo check --all-targets`;
3. proceed to and pass `cargo clippy --all-targets -- -D warnings`.

Record the workflow run URL, macOS job URL, tested commit or test-merge SHA, runner label, and both Rust step conclusions. If #1156 is not yet in the target branch, do not edit the workflow in #1208; final acceptance waits for a GitHub revision that contains both changes.

#### Windows, unchanged behavior

The existing `windows-latest` `rust-regression` job must pass:

- `cargo check --all-targets`;
- `cargo clippy --all-targets -- -D warnings`;
- `cargo test --lib --bins --tests`.

The test step is the execution evidence that the default-Wry library tests still receive `any_thread` on Windows.

#### Linux, no new regression

The #1156 `ubuntu-latest` job must show:

- no `E0599` or other diagnostic at `test_app_builder` or the eight migrated sites;
- no new test-builder diagnostic relative to the known #1113 inventory.

Unrelated pre-existing Linux clippy failures retain separate ownership and must not be changed in #1208.

## 10. Objective acceptance criteria

Implementation is accepted only when every item below is true:

1. One and only one crate-root `#[cfg(test)]` `test_app_builder` exists with the exact Windows/Linux predicate and complementary default branch.
2. All eight macOS-visible library-test builders call `crate::test_app_builder()`.
3. Windows and Linux select `Builder::default().any_thread()`; macOS selects `Builder::default()`.
4. The four Windows-only integration-test calls and both files are unchanged.
5. The application-source diff contains exactly the five files in Section 6, plus the plan artifact outside application source.
6. No production construction, runtime generic, MockRuntime, dependency, feature, workflow, command, test filter, ignore, allow, or warning-policy change exists.
7. Frontend assets build, rustfmt check passes, all-target check passes, clippy passes with `-D warnings`, and diff hygiene passes.
8. GitHub macOS evidence shows both all-target check and clippy green on the reviewed source revision.
9. GitHub Windows evidence remains green through `cargo test --lib --bins --tests`.
10. Linux shows no new builder-related regression; unrelated diagnostics remain outside this issue.
11. The final exact-text audit accounts for every remaining `.any_thread()` occurrence under Tauri's supported predicate or a crate-level Windows predicate.

## 11. Reclassification and stop conditions

Stop the Lite implementation and report a Full reclassification blocker if any required fix expands into:

- runtime-generic `AppHandle`, `GitWatcher`, `WsState`, `PtyManager`, or production state;
- `MockRuntime` conversion of the affected helpers;
- production `tauri::Builder` or application-construction changes;
- actual Wry test execution on macOS or a custom main-thread harness;
- a dependency, feature, workflow, warning-policy, test-filter, ignore, or allow change.

An unrelated macOS clippy diagnostic is a separate issue blocker, not permission to expand #1208. An unrelated Linux diagnostic remains with #1113.
