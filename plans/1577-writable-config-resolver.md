# #1577 — Writability-aware config resolver with portable marker

Status: READY_FOR_IMPLEMENTATION

## Delivery identity

- Issue: [#1577](https://github.com/mblua/AgentsCommander/issues/1577), including approved specification comment `#issuecomment-5444596303`.
- Delivery path: Full.
- Target: `main` pinned at `1eee2cd72a0d25095108d92b3f495da84b979d24`.
- Branch: `fix/1577-writable-config-resolver`, created from that exact target.
- Planned application/test paths, and no others:
  - `src-tauri/src/config/mod.rs`
  - `src-tauri/src/lib.rs`
  - `src-tauri/src/main.rs`
  - `scripts/smoke-cli-release-windows.ps1`
- Plan artifact: `plans/1577-writable-config-resolver.md`.
- Task class: routine application and test change with elevated state-location safety impact. The accepted threat model covers accidental state relocation, real OS writability/ACL behavior, incomplete probe cleanup, and startup failure visibility. It does not include hostile-host/toolchain provenance, malicious `PATH`, symlink/reparse adversaries, signing, packaging provenance, destructive migration, or an untrusted build host.

## Objective and cause

Make an unmarked executable-adjacent config location conditional on an actual successful write probe, add a supported release-build config override, make `portable.txt` an explicit no-fallback assertion, and replace the app-outbox panic with a visible exit-1 startup error.

Evidence at the pinned base:

- `src-tauri/src/config/mod.rs:87-155`, `resolve_instance_location`, always selects `<exe_parent>/.<exe_stem>` when `current_exe()` has a parent/stem. It never considers writability; `$HOME/<profile::config_dir_name()>` is reached only after an unusable executable result.
- `src-tauri/src/config/mod.rs:158-168`, `instance_location`, owns the process-lifetime `OnceLock` and reads only debug-only `AGENTSCOMMANDER_TEST_CONFIG_DIR` before calling the pure resolver.
- `src-tauri/src/config/mod.rs:186-195`, `config_dir` and `instance_base`, and `:177-179`, `agent_local_dir_name`, are projections from that cached location.
- `src-tauri/src/lib.rs:1823-1870`, `run`, resolves config, performs early startup work, then panics at `create_dir_all(&app_outbox_path).expect(...)` when the chosen state root cannot host the app outbox.
- `src-tauri/src/cli/mod.rs:334-353` already provides `present_fatal_startup_message(&str)` on Windows and non-Windows. It returns `()` and never exits; its caller owns termination.
- The nine resolver tests at `src-tauri/src/config/mod.rs:208-359` pin every existing debug-override, executable, fallback, stem, and instance-base result.

## Scope

In scope:

- The exact precedence and behavior below on Windows, macOS, and Linux.
- Real marker and create/write/delete probes owned by the config cache initializer.
- A typed, cached portable-marker startup error without changing existing config projections.
- A typed `lib::run` startup result, visible reporting in `main`, and exit code 1.
- Focused unit tests and the already-triggered Windows release smoke.

Out of scope:

- #1137's exact Linux `/usr/bin/agentscommander` identity, XDG target/input/error, fallible config-dir projection, and reserved precedence slot 3.5.
- #1594's macOS bundle-container target and any genuine portable AppImage rule.
- `packaging/**`, `.github/workflows/**`, `npm/**`, `scripts/pack-windows-portable.mjs`, and `scripts/smoke-windows-portable.ps1`.
- `README.md`, `docs/quickstart.md`, `docs/features/portable-instances.md`, and `PRIVACY.md`.
- Migration, copying, cleanup, or reconciliation between executable-adjacent and home state.
- XDG, canonicalization, permission-bit inspection, ACL mutation, UAC configuration, AppImage heuristics, and a new fatal-message surface.
- General removal of unrelated startup `expect` calls. In particular, the existing no-home `config_dir().expect("Cannot determine home directory")` remains distinct; #1577 preserves the resolver's `config_dir: None` result when both executable and home inputs are unavailable.
- Cargo/package/lockfile changes, new dependencies, unrelated cleanup, and module reorganization.

The release owner must relay the documentation claim delta: an unmarked copy is executable-adjacent only while its real probe succeeds; `portable.txt` asserts executable-adjacent state and refuses fallback. Documentation and packaging changes remain release-track owned.

## Required resolution contract

First match wins. `std::env::var` supplies the two environment values; non-Unicode values remain unset, matching today's `.ok()` behavior. A Unicode value is unset only when `raw.trim().is_empty()`, but every selected path uses the original untrimmed string verbatim.

1. Nonblank `AGENTSCOMMANDER_CONFIG_DIR`, in debug and release: `PathBuf::from(raw)` verbatim. An absolute path exposes its parent as `instance_base`; a relative path exposes no base. `local_dir_stem` still comes from `current_exe()` or the existing `agentscommander` default.
2. Nonblank `AGENTSCOMMANDER_TEST_CONFIG_DIR`, compiled/read only under `debug_assertions`, with identical path/base/stem semantics. A public override can never be shadowed by it.
3. When `<exe_parent>/portable.txt` exists, select `<exe_parent>/.<exe_stem>`. A successful write probe confirms startup may continue. A failed write probe produces the typed hard error below and never considers home.
4. Without the marker, select `<exe_parent>/.<exe_stem>` only when its write probe succeeds. Preserve the current relative-path behavior and expose `instance_base` only when the executable parent is absolute.
5. Without the marker, any write-probe failure, `current_exe()` failure, or executable path without parent/stem selects `$HOME/<profile::config_dir_name()>` with `instance_base: None`.
6. If that fallback also lacks a home directory, retain `config_dir: None` and `instance_base: None`.

Both override branches short-circuit lower I/O: they do not inspect `portable.txt`, create the adjacent directory, or create a probe file. Invalid executable shapes likewise skip both probes.

## Probe ownership, protocol, and caching

`instance_location()` remains the only production I/O/cache owner. Inside its existing `OnceLock` initializer it must:

1. Capture public override, debug override, `current_exe()`, and `dirs::home_dir()` exactly once.
2. Reuse one private nonblank-override predicate to decide whether executable probes are needed; this predicate must be the same one the pure resolver uses for precedence.
3. If no override wins and the captured executable has a parent/stem, derive the marker and candidate paths through one shared pure path helper, evaluate the marker probe, and execute the write probe. The resolver uses that same helper against the captured executable, preventing probe/selection path drift.
4. Pass the captured values and typed marker/write-probe outcomes into `resolve_instance_location`. That resolver must contain no `std::fs` or environment access; tests inject outcomes just as they inject `current_exe_result` and `home_dir` today.
5. Cache the complete `InstanceLocation`, including its optional startup error. Never re-read environment values, re-run probes, or change roots during the process.

The private probe outcome types must distinguish marker present/absent/not-run and write writable/failed/not-run; a write failure carries the failing operation, affected path, `io::ErrorKind`, raw OS code when available, `io::Error::to_string()`, and an optional cleanup error. Override and invalid-executable branches may carry not-run outcomes because the resolver never consumes them there.

The resolver's exact signature shape is:

```rust
pub(crate) fn resolve_instance_location(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    marker_probe: MarkerProbeOutcome,
    write_probe: WriteProbeOutcome,
) -> InstanceLocation
```

Both outcome enums are private values with `NotRun` variants; neither contains a callback or performs I/O.

The real write probe for candidate directory `D` is exactly:

1. `create_dir_all(D)`.
2. Open `D/.agentscommander-write-probe-<Uuid::new_v4()>.tmp` with `write(true)`, `create_new(true)`, and no truncate/append.
3. `write_all(b"AgentsCommander write probe\n")`.
4. Close the handle.
5. `remove_file` that exact probe file.

`uuid` is already a production dependency with v4 support; no dependency or lockfile edit is allowed. `sync_all` is deliberately absent because this probe establishes mutation authority, not durability.

If `D` did not exist, a successful probe creates and retains it as the selected state directory; only the probe file is removed. After an open/write failure, close the handle and best-effort remove only the exact probe file this invocation created. Preserve the first required-operation failure; if cleanup also fails, append `; cleanup of probe file "{probe_path}" also failed: {cleanup_os_reason}`. Never recursively delete `D`, its ancestors, or user content.

Every `io::ErrorKind` from directory creation, probe-file creation, write, or deletion means the probe did not prove writability. There is no permission-only allowlist and no fatal "other kind" for an unmarked copy:

- unmarked candidate: any failure selects the home fallback;
- marker present: any failure is the typed hard startup error;
- delete failure after successful create/write also fails the probe because create/write/delete is the accepted contract.

The cached selection is stable. If an unmarked candidate becomes writable later, the process stays on home until restart. If a selected adjacent/override directory becomes unwritable later, the app-outbox creation path returns the second startup error. A new process re-evaluates everything.

## `portable.txt` semantics

- The marker path is the exact native path component `<exe_parent>/portable.txt`.
- Its contents are never opened or interpreted; an empty file is sufficient.
- Any entry for which `Path::exists()` is true counts, including a directory. A broken/unresolvable link follows `Path::exists()` and counts as absent; no third marker-metadata error surface is introduced.
- No manual case folding occurs. Windows follows native case-insensitive lookup; case-sensitive filesystems require the exact spelling.
- Marker presence changes placement/failure policy only. It does not alter `local_dir_stem`, `instance_base`, file formats, instance identity, ports, or config contents.

## Typed errors and process boundary

In `config/mod.rs`:

- Add cloneable `ConfigWriteProbeFailure` data and `ConfigStartupError::PortableDirectoryUnwritable { marker_path, config_dir, failure }`.
- Add a private `portable_startup_error: Option<ConfigStartupError>` field to `InstanceLocation`.
- Marker plus a successful probe returns the normal adjacent location with no error. Marker plus a failed probe returns the same adjacent `config_dir`, stem, and absolute-only base plus the error. It must never construct a home location.
- Keep `config_dir() -> Option<PathBuf>`, `instance_base() -> Option<PathBuf>`, and `agent_local_dir_name() -> String` unchanged. Add only crate-private `portable_startup_error() -> Option<ConfigStartupError>` as a clone projection.

This narrow error projection is not #1137's future general fallible config projection. #1137, when replanned on this resolver, owns the reserved branch between marker and normal probe, its XDG inputs/error, and a process-wide `try_config_dir`-style contract. #1577 must not add any exact Linux-package classifier or name/claim that future projection.

In `lib.rs`:

- Add public `StartupError` with private typed variants for the config portable error and `AppOutboxCreate { config_dir, app_outbox_path, source: io::Error }`; implement `Display` and `Error`.
- Change `run(test_window_placement, ui_automation_enabled)` from `()` to `Result<(), StartupError>`.
- Before `logging::init_logger`, token generation, settings access, or application-state mutation, return the cached portable error if present.
- Replace only the `create_dir_all(&app_outbox_path).expect(...)` with `map_err` into `AppOutboxCreate` and `?` at the same ownership point.
- Return `Ok(())` after the Tauri run loop ends.

In `main.rs`, handle only the GUI `run` result: on `Err`, call `agentscommander_lib::cli::present_fatal_startup_message(&error.to_string())` exactly once, then `std::process::exit(1)`. Existing parse/validation, CLI-command, UI-automation, and second-instance priority/exit behavior remains unchanged.

Exact user-facing templates (paths use `.display()`; the OS-controlled reason remains verbatim):

```text
AgentsCommander cannot start because portable marker "{marker_path}" requires configuration directory "{config_dir}", but its write probe failed: {probe_reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.
```

`{probe_reason}` is `failed to {create configuration directory|create probe file|write probe file|delete probe file} "{affected_path}": {os_reason}`, followed by the cleanup suffix above only when applicable.

```text
AgentsCommander cannot start because it could not create app outbox directory "{app_outbox_path}" for configuration directory "{config_dir}": {os_reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.
```

Neither error prints a Rust panic/backtrace hint. Windows presentation continues to follow #1592's stdout/stderr/dialog destination contract; non-Windows writes and flushes stderr. The helper never exits; `main` owns exit 1.

## Platform, compatibility, and security behavior

- Windows, macOS, and Linux run the same marker and write-probe protocol. No platform `cfg` narrows it.
- A writable portable zip/USB/user-folder copy remains adjacent, including renamed executables and their per-stem isolation. A root-owned/system install falls back to home when unmarked. `portable.txt` converts failure into a visible refusal to relocate state.
- A macOS in-bundle executable that fails the probe falls back to home unless marked; this is the safety net, not #1594's containing-`.app` placement.
- A read-only Linux AppImage mount falls back to home unless marked and therefore works as a per-user install, not a genuinely portable AppImage. No `$APPIMAGE` rule is added.
- `$HOME/<profile::config_dir_name()>` remains byte-for-byte the fallback construction. No XDG path is introduced.
- Existing config schemas, file contents, instance name/stem, port derivation, and absolute-only `instance_base` contract are unchanged.
- Overrides intentionally preserve the existing verbatim relative-path/CWD semantics; no canonicalization or validation is added.
- `create_new` and a unique name prevent probe truncation/clobber. Probe bytes contain no secret. The cleanup is scoped to the exact file created by this process.
- The config layer remains filesystem/path-only and gains no `cli`, Tauri, `AppHandle`, or UI-transport dependency. Presentation and process exit stay at the existing executable/application boundary.
- Symlink/reparse no-follow hardening, ACL manipulation, host executable hashes, helper inventories, and custom cross-process locks are non-applicable enhanced controls under the accepted threat model. Unique probe files make concurrent probes independent; the existing singleton remains unrelated to resolver correctness.

## Affected files and symbols

1. `src-tauri/src/config/mod.rs`
   - `InstanceLocation`, new probe/error data, marker and real write-probe helpers.
   - `resolve_instance_location` inputs and precedence.
   - `instance_location` production I/O and `OnceLock` initialization.
   - New crate-private `portable_startup_error`; existing three projections unchanged.
   - Preserve all nine resolver tests and add focused #1577 unit/probe/error tests.
2. `src-tauri/src/lib.rs`
   - New `StartupError` and formatting tests.
   - `run` result, pre-logger portable-error return, fallible app-outbox creation, terminal `Ok(())`.
3. `src-tauri/src/main.rs`
   - GUI call-site handling, existing fatal-message helper, and exit 1.
4. `scripts/smoke-cli-release-windows.ps1`
   - Add bounded release-process cases for marker hard failure and public override precedence/outbox failure, reusing the script's existing release binary, log root, and process cleanup conventions.

No module/file move, Cargo feature/dependency, manifest, lockfile, frontend, IPC, persistence schema, workflow, packaging, npm, or documentation edit is allowed.

## Ordered implementation

1. Freeze branch/base/index/tracked/untracked state and the four application/test paths. Stop on any unrelated dirty state or relevant target drift.
2. In `config/mod.rs`, add typed probe outcomes/failures, exact `Display` text, marker semantics, and the real create/write/delete helper using only std plus existing `uuid`.
3. Extend `InstanceLocation` and the pure resolver with public/debug override inputs and injected marker/write outcomes. Preserve all old fields/results, implement the six-tier precedence, and expose only `portable_startup_error` in addition to existing projections.
4. Make `instance_location` capture all runtime inputs once, short-circuit lower probes for overrides/invalid executable shapes, run required I/O, and cache the final location/error for process lifetime.
5. In `lib.rs`, add `StartupError`, make `run` fallible, preflight the cached marker error before logging/state work, and replace the app-outbox `expect` with the typed result.
6. In `main.rs`, present a failed GUI `run` through the existing helper and exit 1 without changing any earlier branch.
7. Add/adjust unit tests, keeping every existing resolver assertion. Extend the Windows release smoke with isolated child environments, bounded waits, and durable diagnostics.
8. Format, run focused and full local validation, run the dependency gate, verify exact diff/path/lockfile/workflow scope, then deliver by PR. No implementation commit or PR may include the plan-only commit's unrelated worktree artifacts.

## Tests and objective acceptance

### Resolver and probe tests

- Retain all nine named resolver tests and every assertion. Only calls gain public override plus injected probe inputs; executable-adjacent rows inject marker absent/write success.
- Public nonblank override beats a simultaneous nonblank debug override, present marker, and failed write outcome; assert absolute and relative path/base semantics and executable-derived stem.
- Blank public override allows nonblank debug override; blank public and blank debug continue to the marker/probe tiers.
- Marker present plus write success selects executable-adjacent state. Marker present plus a synthetic `PermissionDenied` returns adjacent state plus the typed error and never the supplied home path.
- Marker absent plus the same failure selects the exact home fallback/no base; marker absent plus success remains adjacent.
- `current_exe()` failure and invalid parent/stem do not consume probe outcomes and preserve current fallback/default-stem results.
- Real probe tests cover an existing directory and a missing directory, verify success, verify no probe file remains, and verify a newly created selected directory remains. The fixed sentinel bytes and `write_all` call are pinned by the helper's focused source/unit contract without introducing an injectable filesystem abstraction.
- A deterministic non-directory/blocking-path test exercises a real failure and captures operation/path/kind/OS reason. A synthetic typed-failure formatting test covers first-failure preservation plus the exact cleanup suffix.
- Marker helper tests prove an empty file, arbitrary-content file, and directory all count; a missing/broken target does not. Source review asserts no content read and no manual case folding.
- Exact formatter tests use fixed paths and synthetic OS reasons to compare both complete startup strings byte-for-byte.

### Startup and release smoke

Extend `scripts/smoke-cli-release-windows.ps1` with uniquely named copies of the already-built release executable so no installed/running identity can interfere:

1. Marker-hard-error case: create an empty adjacent `portable.txt`, make the computed adjacent config candidate deterministically fail the real probe, clear both config override variables, set child-only `AC_UI_AUTOMATION=0`, remove child-only `AC_TEST_WINDOW_PLACEMENT`, and launch `--app` with stdout/stderr redirected. Require exit 1, marker path, adjacent config path, OS reason, and `AGENTSCOMMANDER_CONFIG_DIR` in the presented error; forbid a home/outbox fallback and `panicked at`.
2. Release-public-override case: retain the marker and failing adjacent candidate, set child-only `AGENTSCOMMANDER_CONFIG_DIR` to a different absolute blocking path, also set `AGENTSCOMMANDER_TEST_CONFIG_DIR` to a third path, and launch `--app`. Require exit 1 and the app-outbox template naming the public override config/outbox path and OS reason; forbid the marker error, debug path, adjacent candidate, and `panicked at`. This proves the public variable is compiled/read in release and outranks all lower tiers.

Each child has a 15-second deadline. On timeout, terminate it, wait for terminal process state, retain stdout/stderr/exit/timing/path diagnostics under ignored `artifacts/cli-release-smoke`, and fail. Environment changes are child-scoped; the script restores or never mutates its own process environment. Test copies/probe artifacts are removed only when they remain inside the case-specific artifact root; logs survive failures for CI upload.

### Required commands

From the repository root, using locked dependencies and the repository/CI-defined toolchain:

```powershell
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib issue_1577_ -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test loops_layering
cargo test --manifest-path src-tauri/Cargo.toml --test instance_gitignore_layering
cargo test --manifest-path src-tauri/Cargo.toml --test project_settings_layering
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib --bins --tests
npm run build:prod:no-bundle
npm run smoke:cli-release-windows
```

Focused failures, formatter drift, an old resolver assertion change/removal, a smoke timeout, exit other than 1, wrong selected path, missing message field, panic text, probe residue, or any unexpected file change blocks delivery. Existing documented test debt must be identified by its exact established signature; no new failure is accepted as debt.

## Dependency-cycle and layering gate

Planned new/removed internal Rust module arcs: zero.

- `src-tauri/src/lib.rs -> crate::config` already exists at the current `config_dir` call sites.
- `src-tauri/src/main.rs -> agentscommander_lib::run` and `-> agentscommander_lib::cli` already exist, including the current fatal-message call.
- All new config probe/error references are internal to `agentscommander_lib::config`; std/uuid references are external dependencies, not Rust module arcs.
- No lower layer gains UI transport. `config` returns typed data; `lib/main` own presentation and termination.

Clean-base `rust-levelization-run` evidence at `1eee2cd72a0d25095108d92b3f495da84b979d24`: 191 modules, 1,037 unique arcs/3,732 sites, 107 SCCs, exactly one cyclic SCC with 85 members. Regenerated `src-tauri/module-arcs.txt` was byte-identical to the tracked 82,149-byte/1,037-arc record, SHA-256 `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`.

The implementation reviewer must run the authoritative detector on clean base and final-head trees. From `repo-AgentsCommander` inside the workgroup, the team-owned tools are at the exact sibling-repository path below:

```powershell
# Run separately in clean base and final-head worktrees.
node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\Levelization\02-levelize.mjs" rank graph.json
node scripts/02-module-arc-record.mjs --graph graph.json --out src-tauri/module-arcs.txt
```

Detector exit 1 is normal when the existing cycle is measured and the graph exists; exit 3 is unusable and blocks. Green requires all of:

1. `coverage.graphShape.cyclicSccs` remains 1.
2. The single 85-module cyclic SCC member set is identical set-to-set, not merely equal in count.
3. The final graph has zero new `from -> to` pairs crossing a previously clean SCC boundary; expected new internal module arcs are also zero.
4. Regenerated `src-tauri/module-arcs.txt` is byte-identical to the committed record and leaves empty Git status for that path.
5. `loops_layering`, `instance_gitignore_layering`, and `project_settings_layering` remain green.

Any new arc, changed SCC membership/count, role inversion, or arc-record byte drift is implementation deviation and returns to architecture review.

## Delivery and nonfunctional gates

### CI parity and deterministic tools

The pinned workflows make these PR jobs applicable: `test-debt`; Windows `rust-regression`; `rust-regression-linux`; `rust-regression-macos`; the four-host `terminal-snapshot-portable` matrix; `windows-release-cli-smoke`; `frontend-regression`; and `lockfile-drift` (whose changed-input detector must report no package-manifest change). `bundle-validation` and `version-sync` path filters do not match the frozen path set. Re-derive this list if the workflows or final diff drift.

CI uses Node 22, npm 11.6.2, Rust stable, `npm ci`, and Cargo's committed lockfile. Local evidence may use the resolved standard toolchain but must record versions; the planning host resolved Rust/Cargo 1.97.1, Node 24.13.0, and npm 11.6.2. Host-dependent Windows presentation/release evidence belongs to `windows-release-cli-smoke`. Every triggered and configured-required check must succeed on the exact PR-head SHA; another SHA, unexplained skip, bypass, or waiver does not satisfy delivery.

### Git, drift, mutation, and recovery

- Before first product write and again before PR creation/update, fetch live `main` and classify drift from pinned base by changed paths and semantic relevance. Refresh only affected source/test/workflow/toolchain evidence; unrelated target movement does not restart accepted design.
- Require the exact issue branch, synchronized/recorded base, clean index, no unrelated tracked edits, and a frozen intended path set. Product Git mutations occur only in the authorized `repo-AgentsCommander` root. Deliver through a PR; never directly push `main`.
- Immediately before each edit, recheck branch/head/index and the affected path bytes. Preserve external edits. Recovery may unstage/remove only this run's newly created or demonstrably unchanged output; do not use broad reset, checkout, restore, clean, or repository-wide deletion.
- Runtime smoke artifacts stay under ignored `artifacts/cli-release-smoke`. Build artifacts stay in repository-standard ignored roots. Final evidence must enumerate tracked, staged, ordinary-untracked, lockfile, config, workflow, and intended path state.
- The Markdown plan digest is not an implementation artifact. For any later plan certification, hash the committed blob byte stream, not the CRLF worktree representation.

### Bounded diagnostics and enhanced controls

All tests/builds use runner or CI timeouts and retain exit code/stdout/stderr sufficient to diagnose failures. The new smoke children use the explicit 15-second deadline above; timeout/cancellation is failure, and cleanup failure cannot erase the primary failure.

No enhanced provenance, signing, binary-hash, helper/DLL inventory, hostile-parent environment quarantine, reparse/hardlink proof, custom transaction ledger, or exclusive process lock applies. The task changes ordinary application code, uses repository/CI-defined toolchains, creates only a unique non-secret probe file, and performs no migration or irreversible mutation. Exact-head GitHub CI, scoped diffs, real probe tests, and no-clobber cleanup are proportionate evidence.

## Ready/acceptance verdict

Implementation is accepted only when the exact precedence, cache lifetime, marker semantics, error templates, exit 1, old resolver outcomes, four application/test files plus this plan, release smoke, local checks, zero-arc dependency criterion, and exact PR-head CI checks all pass. Any need to change an approved precedence/result, add packaging/npm/workflow/docs work, add #1137/#1594/AppImage behavior, or weaken marker hard failure is a blocker back to the release owner, not an implementer decision.
