# #1577 — Writability-aware config resolver with portable marker

Status: READY_FOR_IMPLEMENTATION

## Delivery identity

- Issue: [#1577](https://github.com/mblua/AgentsCommander/issues/1577), approved specification comment `#issuecomment-5444596303`, and the release-owner round-two rulings that extend it.
- Delivery path: Full.
- Target: `main` pinned at `1eee2cd72a0d25095108d92b3f495da84b979d24`.
- Branch: `fix/1577-writable-config-resolver`, created from that exact target.
- The release owner and both reviewers already classified later `origin/main` drift as bounded and semantically non-blocking. The pinned base stands; there is no round-two rebase gate.
- Planned application/test paths, and no others:
  - `src-tauri/src/config/mod.rs`
  - `src-tauri/src/config/profile.rs`
  - `src-tauri/src/config/settings.rs`
  - `src-tauri/src/cli/mod.rs`
  - `src-tauri/src/lib.rs`
  - `src-tauri/src/main.rs`
  - `scripts/smoke-cli-release-windows.ps1`
- Plan artifact: `plans/1577-writable-config-resolver.md`.
- Task class: routine application/test work with elevated state-location safety impact. The accepted threat model covers accidental state relocation, real ACL/read-only behavior, short-lived Windows antivirus/file-sharing interference, ambiguous marker metadata, probe debris, and startup failure visibility. It does not include hostile-host/toolchain provenance, malicious `PATH`, signing/packaging provenance, destructive migration, or an untrusted build host.

## Objective and verified cause

Make an unmarked executable-adjacent config location conditional on a successful real write probe, add a supported release-build override, make `portable.txt` a hard no-fallback assertion, and replace the app-outbox panic with a visible exit-1 startup error.

Evidence at the pinned source tree:

- `src-tauri/src/config/mod.rs:87-155`, `resolve_instance_location`, always selects `<exe_parent>/.<exe_stem>` when the executable has a parent/stem. It never checks writability.
- `src-tauri/src/config/mod.rs:158-168`, `instance_location`, owns the process-lifetime `OnceLock` and performs the resolver's explicit `current_exe()` capture.
- The home branch calls `profile::config_dir_name()`. At `profile.rs:45-56` that calls `binary_suffix()`, whose private `OnceLock` at `profile.rs:20-30` performs another `current_exe()` call. The round-one six-argument resolver was therefore not transitively deterministic.
- `binary_suffix()` splits the executable stem at the first underscore; `config_dir_name`, title, mutex, instance label, and both port derivations depend on that behavior. This parsing contract must not change.
- `src-tauri/src/config/settings.rs:4413-4452` implements #1436: Windows raw OS 5 and 32 retry with delays `[15, 30, 60, 120, 240]` before the final attempt. The writable-config probe must share this policy, not copy it.
- `src-tauri/src/cli/mod.rs:579-698` already treats window/UI automation controls as hidden release-parsed test flags and pins both hidden-help and parse behavior. The controlled-home acceptance input follows this existing testability surface.
- On Windows, `dirs::home_dir()` uses `SHGetKnownFolderPath(FOLDERID_Profile)`, not `USERPROFILE`/`HOME` ([`dirs` source contract](https://docs.rs/dirs/latest/dirs/fn.home_dir.html)). A child environment block cannot control this acceptance seam; the round-two plan therefore uses an explicit, hidden, validated test-home input rather than making a false environment claim.
- `src-tauri/src/lib.rs:1823-1870` initializes logging/state, obtains `config_dir()`, and panics when creating `<config>/instances/<uuid>/outbox`.
- `src-tauri/src/main.rs:5-152` already owns GUI process exit and calls both `agentscommander_lib::run` and `cli::present_fatal_startup_message`.
- The nine resolver tests at `config/mod.rs:208-359` pin every existing override, executable, home, stem, and absolute-only instance-base result.

## Scope and exclusions

In scope:

- The precedence, marker states, retry/classification policy, cleanup semantics, and cache lifetime below on Windows, macOS, and Linux.
- A deterministic profile derivation over the resolver's one captured executable without changing first-underscore parsing or downstream title/mutex/port behavior.
- One shared Windows retry policy used by settings replacement and the config probe.
- One hidden release-smoke-only `--test-home-dir` input, validated at the executable boundary and used only as tier 5/6's home input.
- Typed cached startup failure and fallback diagnostics without changing existing config projections.
- A typed `lib::run` result, fatal presentation in `main`, focused tests, and the already-triggered Windows release smoke.

Out of scope:

- #1137's Linux package classifier, XDG target/input/error, general fallible config projection, and reserved precedence slot 3.5.
- #1594's macOS bundle-container target and any genuine portable AppImage rule.
- Packaging, npm, workflow, manifest, lockfile, frontend, schema, migration, documentation, and unrelated startup-`expect` changes.
- Canonicalization, permission-bit inspection, ACL mutation in product code, UAC configuration, symlink/reparse hardening beyond the marker-state decision below, and a new fatal-message surface.
- A public/general home override or environment variable. `--test-home-dir` is hidden, cannot select a config directory directly, and is accepted only by the issue-specific copied smoke executable in GUI mode.
- The existing `config_dir().expect("Cannot determine home directory")` when both executable and home inputs are unavailable. Tier 6 preserves that existing `None` result.

The release owner must relay the documentation claim delta: an unmarked copy is adjacent only while the real probe succeeds; `portable.txt` forbids fallback. Documentation and packaging remain release-track owned.

## Required resolution contract

`std::env::var` supplies both overrides. Non-Unicode values remain unset. A Unicode value is unset only when `raw.trim().is_empty()`; a selected path uses the original untrimmed string verbatim.

First decisive state wins:

1. Nonblank `AGENTSCOMMANDER_CONFIG_DIR` in debug and release selects `PathBuf::from(raw)` verbatim. Absolute paths expose their parent as `instance_base`; relative paths expose no base. The executable still determines `local_dir_stem`.
2. Nonblank `AGENTSCOMMANDER_TEST_CONFIG_DIR`, compiled/read only under `debug_assertions`, has the same path/base/stem semantics. The public override always wins.
3. With no override and a usable executable, inspect `<exe_parent>/portable.txt`:
   - marker indeterminate is a hard startup error; do not probe the candidate or consider home;
   - marker present plus write success selects `<exe_parent>/.<exe_stem>`;
   - marker present plus any exhausted write-probe failure is a hard startup error and never considers home.
4. Marker absent plus write success selects the executable-adjacent candidate, retaining today's relative-path and absolute-only-base behavior.
5. Marker absent plus a conclusively-unwritable exhausted probe selects `$HOME/<the profile config name derived from the same captured executable>` with no base. `current_exe()` failure or a path without parent/stem also reaches this home tier without marker/write I/O.
6. If the applicable home fallback has no home directory, preserve `config_dir: None` and `instance_base: None`.

Marker absent plus an indeterminate exhausted probe is not tier 5: it is a hard startup error with no relocation. Slot 3.5 stays reserved for #1137.

Both override branches short-circuit marker and write I/O. Invalid executable shapes skip both probes. The process never re-probes or changes root after `instance_location()` initializes.

## One executable capture and transitive purity

Round two deliberately drops round one's exact six-argument claim. The resolver takes one additional already-derived profile-name input so its legacy home assertions remain unchanged without a hidden executable query:

```rust
pub(crate) fn resolve_instance_location(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    fallback_config_dir_name: &str,
    marker_probe: MarkerProbeOutcome,
    write_probe: WriteProbeOutcome,
) -> InstanceLocation
```

`instance_location()` captures its own public override, debug override, and `current_exe()` result once. Its home input is the already-validated smoke test home when supplied, otherwise one `dirs::home_dir()` call. From the captured executable it derives `fallback_config_dir_name` once before calling the resolver. “One capture” is scoped to the instance-location transaction; unrelated existing main/profile consumers are not claimed to share a process-wide capture.

In `profile.rs`:

- Extract a pure executable-path-to-suffix helper from `binary_suffix()`. Preserve exactly: file stem via `to_string_lossy`, first `find('_')`, and all bytes after that first underscore. Do not change empty/multiple-underscore or no-underscore results.
- Keep existing `binary_suffix()`, its `OnceLock`, its external callers, and all title/mutex/instance-label/port outcomes. It delegates parsing to the new pure helper but retains its current capture/cache behavior.
- Add a `pub(super)` pure config-name derivation accepting `Option<&Path>`. It applies the same suffix parser and existing `BUILD_PROFILE` rule and returns the same two static names.
- Make existing `config_dir_name()` delegate name selection through the same pure rule, retaining its signature/cache and behavior.

`resolve_instance_location` joins `home_dir` with the injected `fallback_config_dir_name` and never calls either profile accessor. Its transitive call graph must contain no environment read, filesystem operation, `OnceLock` mutation, UUID/time/sleep, logging, or callback. Given identical seven inputs, it returns identical fields/errors/diagnostics. The production I/O adapter remains `instance_location()`.

Production derives the injected name with the new pure profile helper over `current_exe_result.as_ref().ok()`. Legacy tests pass `profile::config_dir_name()` as that new input, so every existing assertion remains byte-for-byte unchanged while only the call signature grows. This adds `profile.rs` to scope and removes the round-one call-order-dependent second executable query without changing suffix parsing.

### Hidden controlled-home test input

In `cli/mod.rs`, add `test_home_dir: Option<PathBuf>` as `--test-home-dir` with `hide = true` and an absolute-path value parser. It must stay absent from short/long help and parse only as a global GUI test flag.

Before any CLI/GUI branch can consume it, a pure CLI validator requires all three:

1. no subcommand is present;
2. the path is absolute; and
3. the already-captured runtime binary stem starts with `agentscommander_issue1577_`.

Invalid use follows the existing argument/validation stderr-plus-flush exit-1 surface and never initializes config. The valid smoke call passes the owned path through `main -> lib::run`; no process environment variable or config global is mutated.

At the first line of `lib::run`, call a crate-private `config::initialize_instance_location(test_home_dir)`. It initializes the same `OnceLock<InstanceLocation>` that all existing projections read. `Some(path)` replaces only the `dirs::home_dir()` input; it does not create a new precedence tier, cannot beat either config override, cannot bypass marker policy, and cannot choose the adjacent candidate. `None` preserves production behavior. CLI subcommands never call this initializer with a test home.

## Marker probe: present, absent, or indeterminate

Do not use `Path::exists()`. The production marker adapter runs each `symlink_metadata`/follow-up `metadata` call through the shared retry helper defined below, then returns:

- `Absent` only when the marker entry itself returns `NotFound`.
- `Present` for a regular file or directory.
- For a symlink, follow it with `metadata`: a resolvable regular-file/directory target is `Present`; a broken link, loop, unreadable target, or ambiguous target is `Indeterminate`.
- A non-file/non-directory/non-symlink entry is `Indeterminate` as an ambiguous entry type.
- Any metadata error other than entry-level `NotFound` is `Indeterminate` after its applicable retries and retains operation, path, attempts, kind, raw OS code, and OS reason.
- `NotRun` is valid only for higher overrides or unusable executable shapes.

Marker contents are never opened. Native case semantics apply. Marker indeterminate uses the same typed hard-error surface as marker-present probe failure, includes the marker path, and never enters the unmarked branch.

Marker-metadata reasons are exact: `could not inspect portable marker entry metadata "{affected_path}" after {attempts} attempt(s): {os_reason}` or `could not resolve portable marker symlink target metadata "{affected_path}" after {attempts} attempt(s): {os_reason}`. An entry/target whose metadata is neither file, directory, nor symlink uses `filesystem metadata reported an unsupported portable marker entry type` as its deterministic reason.

The round-one planned assertion “a broken marker link is absent” is explicitly inverted: `broken_marker_link_is_indeterminate_and_never_falls_home` must assert the hard error, no write probe consumption, and no home selection. It must not be retained, renamed to preserve the old outcome, or deleted.

## Shared retry policy and final classification

Put one test-injectable transient-I/O retry policy in `config/mod.rs` and make both the config probe and `settings.rs`'s existing atomic-replace wrapper delegate to it.

`settings.rs` is a necessary additional application/test path. Excluding it would require a second transient predicate/backoff table and violate the release-owner ruling that #1436 and the probe cannot drift.

- Move/generalize the sole Windows backoff constant to the config root: exactly `[15, 30, 60, 120, 240]` ms.
- On Windows, raw OS 5 (`ERROR_ACCESS_DENIED`) and 32 (`ERROR_SHARING_VIOLATION`) receive the existing five sleeps and final sixth attempt. Existing #1436 call/sleep/error behavior and tests remain green.
- `ErrorKind::Interrupted` receives one immediate additional attempt on every platform. It consumes no sleep. The budget is per required filesystem operation and cannot reset recursively.
- On non-Windows, every non-`Interrupted` error returns after the first attempt. In particular, `PermissionDenied`/`ReadOnlyFilesystem` receives no Windows-shaped sleep/backoff.
- A chain mixing `Interrupted` with Windows raw 5/32 has at most seven calls: initial, one immediate interrupted retry, and the five Windows delayed retries. The returned error is the terminal exhausted error.

Only after that helper returns an error may the probe classify it. The conclusive allowlist is closed:

- Windows raw OS 5 or 32; or
- `ErrorKind::PermissionDenied` or `ErrorKind::ReadOnlyFilesystem` on any platform.

Everything else is `Indeterminate`, including persistent `Interrupted`, unknown raw OS errors, `AlreadyExists`, `NotFound` during the required delete, path-shape errors, storage-full errors, and any future kind not explicitly added to the conclusive set. “When in doubt” means hard startup error, never silent relocation.

## Real write probe and cleanup/debris contract

For candidate directory `D`:

1. Retry-wrapped `create_dir_all(D)`.
2. Retry-wrapped `create_new` of `D/.agentscommander-write-probe-<Uuid::new_v4()>.tmp` with write enabled and no truncate/append.
3. Retry-wrapped `write_all(b"AgentsCommander write probe\n")` on that owned handle.
4. Close the handle.
5. Retry-wrapped `remove_file` of that exact probe file.

`uuid` v4 is already a production dependency. No `sync_all`, permission-bit check, recursive delete, candidate rollback, or new dependency is allowed. A newly created successful candidate remains; the probe file does not.

Cleanup rules:

- No cleanup runs after `create_new` fails because this invocation did not establish file ownership.
- After a post-create write failure, close the handle and run the same retry helper around best-effort removal of only the owned probe file.
- `NotFound` during this best-effort cleanup proves no debris and is cleanup success. `NotFound` during the required step-5 delete remains an indeterminate probe failure.
- Preserve the terminal primary operation/path/attempts/kind/raw-code/OS-reason. If cleanup exhausts, also preserve its complete terminal reason and set `probe_may_remain: true` with the exact probe path.
- An exhausted required step-5 delete also sets `probe_may_remain: true` for that exact path. Do not start another removal loop: the shared helper already consumed the complete retry budget.
- Aggregate classification is conclusive only when every retained non-success error is in the closed conclusive allowlist. Any indeterminate primary or cleanup error makes the whole outcome indeterminate.
- A successful cleanup removes its transient errors from the final outcome. An exhausted cleanup is never discarded.

For marker absent plus a conclusive failure, cache an `AdjacentFallbackDiagnostic` alongside the home selection. It includes candidate, selected home (when any), complete primary/cleanup reasons, probe path, and debris flag. After logger initialization, `lib::run` emits one warning from that retained diagnostic. This closes round one's discarded-reason gap while preserving tier-5 startup.

For marker present, any failure is hard regardless of classification. For marker absent, indeterminate is hard. No hard-error path initializes logging or state before returning.

## Cached data, typed errors, and process boundary

In `config/mod.rs`:

- Add cloneable marker/probe failure data, `ProbeFailureClass`, and `ConfigStartupError::AdjacentSelectionBlocked { config_dir, marker_path: Option<PathBuf>, reason }`.
- Extend `InstanceLocation` privately with `startup_error: Option<ConfigStartupError>` and `fallback_diagnostic: Option<AdjacentFallbackDiagnostic>`.
- Put its `OnceLock` behind crate-private `initialize_instance_location(test_home_dir: Option<PathBuf>)` plus the existing no-argument projection path. Once initialized, a later different test-home request is an invariant error in debug/tests and is never silently accepted.
- Keep `config_dir() -> Option<PathBuf>`, `instance_base() -> Option<PathBuf>`, and `agent_local_dir_name() -> String` unchanged.
- Add only crate-private clone projections for the startup error and fallback diagnostic. This is not #1137's future general fallible config API.

Exact startup text without a known marker:

```text
AgentsCommander cannot start because configuration directory "{config_dir}" could not be safely selected: {reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.
```

When marker status is present or indeterminate, append exactly:

```text
 Portable marker path: "{marker_path}".
```

Thus every hard selection message contains the directory, terminal filesystem/OS reason, and `AGENTSCOMMANDER_CONFIG_DIR`; it contains the marker path only for marker-present/indeterminate states. `{reason}` names marker metadata or write-probe operation/path, attempts, OS reason, and any cleanup/debris clause. Paths use `.display()`; OS text remains verbatim, while kind/raw code remain available in typed diagnostics/tests.

Write-probe reasons use exactly `write probe could not {create configuration directory|create probe file|write probe file|delete probe file} "{affected_path}" after {attempts} attempt(s): {os_reason}`. An exhausted post-failure cleanup appends ` Cleanup of probe file "{probe_path}" also failed after {attempts} attempt(s): {cleanup_os_reason}; the probe file may remain.` An exhausted required delete appends ` The probe file "{probe_path}" may remain.`

In `lib.rs`:

- Add public `StartupError` with private typed variants for the config startup error and `AppOutboxCreate { config_dir, app_outbox_path, source: io::Error }`; implement `Display`/`Error`.
- Change `run(test_window_placement, ui_automation_enabled, test_home_dir: Option<PathBuf>)` from `()` to `Result<(), StartupError>`.
- First initialize the instance location with that validated test-home input; then, before logger/token/settings/state mutation, return the cached config startup error.
- Initialize the logger, then emit the retained conclusive-fallback warning when present.
- Replace only `create_dir_all(&app_outbox_path).expect(...)` with the typed `map_err`/`?` at the same ownership point.
- Return `Ok(())` after the Tauri run loop.

Exact app-outbox text remains:

```text
AgentsCommander cannot start because it could not create app outbox directory "{app_outbox_path}" for configuration directory "{config_dir}": {os_reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.
```

In `cli/mod.rs`, add the hidden field, absolute-path/parser validation, runtime binary/mode validator, and updates to `internal_verbs_and_test_flags_are_hidden_from_help` plus `hidden_test_only_flags_still_parse`.

In `main.rs`, validate the hidden test-home input against the already-captured binary name and command mode, pass it only to the GUI `run` call, and handle that result. On `Err`, call `agentscommander_lib::cli::present_fatal_startup_message(&error.to_string())` once and exit 1. Other parse, CLI-command, UI-automation, mutex/second-instance, and exit behavior stays unchanged. Neither startup error contains panic/backtrace text.

## Platform and compatibility behavior

- Windows, macOS, and Linux share marker/probe/classification semantics. Only the retry schedule is platform-specific as stated.
- A short raw-32/raw-5 Windows hold that clears within #1436's schedule stays adjacent. Persistent raw 5/32 is conclusively unwritable and reaches home when unmarked.
- Unix `EACCES`/read-only results classify immediately without sleep. Unknown Unix errors hard-stop.
- Writable zip/USB/user-folder copies remain adjacent, including renamed executable isolation. Root/system installs fall home when unmarked. A marker converts any inability/ambiguity into visible refusal.
- macOS bundle and Linux AppImage behavior remain safety nets, not #1594/#1137 placement implementations.
- Existing schemas, contents, stem, mutex, labels, ports, absolute-only base, and verbatim relative override behavior remain unchanged.
- Product code never changes ACLs. `create_new` plus UUID prevents truncation/clobber. Cleanup is limited to the file this process created.
- `config` gains no CLI/Tauri/`AppHandle`/UI dependency. Presentation and process termination stay at `lib/main`.

## Affected files and symbols

1. `src-tauri/src/config/mod.rs`
   - Shared retry policy, marker/probe outcomes and adapters, classification, cleanup, typed errors/diagnostics.
   - Seven-input pure resolver and one-shot `instance_location` orchestration.
   - Unchanged public projections plus narrow startup/diagnostic projections.
   - Existing nine tests retained; focused #1577 tests added.
2. `src-tauri/src/config/profile.rs`
   - Pure suffix/config-name derivation; existing `binary_suffix`/`config_dir_name` delegate without changing first-underscore behavior.
3. `src-tauri/src/config/settings.rs`
   - Existing Windows atomic-replace wrapper delegates to the shared retry policy; #1436 behavior/tests stay intact.
4. `src-tauri/src/cli/mod.rs`
   - Hidden absolute `--test-home-dir` field, runtime smoke-identity/mode validation, help/parse tests.
5. `src-tauri/src/lib.rs`
   - `StartupError`, early config failure, retained fallback warning, fallible outbox create, `run -> Result`.
6. `src-tauri/src/main.rs`
   - Hidden test-home validation/pass-through, GUI result presentation, and exit 1.
7. `scripts/smoke-cli-release-windows.ps1`
   - Three bounded release-child cases, including the real unmarked fallback seam.

No module/file move, new source file, dependency, lockfile, frontend, IPC, persistence schema, workflow, packaging, npm, or documentation edit is allowed.

## Ordered implementation

1. Recheck branch/base/index/affected bytes and freeze the seven-path scope. Stop on unrelated dirty state.
2. Split profile capture from derivation and pin first-underscore behavior before changing the resolver call.
3. Generalize the #1436 retry policy in the config root; delegate settings replacement and prove its old tests unchanged.
4. Add marker tri-state, write probe, final classification, cleanup/debris data, and retained diagnostic.
5. Extend `InstanceLocation` and the pure resolver; preserve all nine old results and implement the decisive-state table.
6. Make `instance_location` perform the single resolver capture, short-circuit I/O, run real adapters once, and cache the result.
7. Add the hidden CLI test-home control and `lib/main` initialization/result handling without moving other startup work.
8. Add focused tests and the three release-smoke cases.
9. Format, validate locally, rerun dependency/scope gates, and deliver through the issue PR with exact-head CI.

## Tests and objective acceptance

### Pure resolver/profile tests

- Prefix every new #1577 unit test, including CLI/config-initializer tests, with `issue_1577_` so the focused command is exhaustive.
- Retain all nine named resolver tests and every assertion; only calls gain public override, `profile::config_dir_name()` as the injected fallback name, and marker/write outcomes.
- Existing adjacent rows inject marker absent/write success. Existing fallback rows retain exact path/stem/base results.
- Public override beats simultaneous debug override, marker, and failed outcomes. Blank values fall through correctly.
- Profile tests pin no underscore, first underscore, multiple underscores, empty suffix, `dev` suffix, and current build-profile fallback. Existing mutex/title/port tests stay unchanged.
- Resolver tests prove marker present success, marker present conclusive/indeterminate failure, marker absent success, marker absent conclusive home fallback, marker absent indeterminate hard error, invalid executable short-circuit, and no-home tier 6.
- Identical seven inputs produce identical complete `InstanceLocation` data; no resolver call installs callbacks or reaches process-global profile caches.
- CLI tests add `--test-home-dir` to the hidden-help allowlist, prove an absolute value parses, and prove the pure runtime validator rejects a relative path, any subcommand, and a non-`agentscommander_issue1577_` binary stem. A config initializer test proves the validated path becomes only the injected `home_dir` input and cannot outrank public/debug/marker tiers.

### Retry, classification, marker, and cleanup tests

- Existing `windows_settings_replace_retries_access_denied` and `...sharing_violation` retain their two-call/`[15]` assertions.
- Shared-helper tests pin six persistent raw-5/raw-32 calls and sleeps `[15,30,60,120,240]`, transient success, one immediate `Interrupted` retry on every platform, the seven-call mixed upper bound, and no Unix sleep for permission/read-only errors.
- Classification table tests prove only the closed allowlist is conclusive and an unknown raw code/future-other kind is indeterminate.
- Real successful probes cover existing/missing directories, retained created directory, and zero probe residue.
- A real non-directory candidate is now asserted indeterminate/hard with no home relocation; it must not masquerade as permission failure.
- Injected cleanup tests cover interrupted-then-success, Windows sharing-violation-then-success, persistent conclusive delete with retained debris diagnostic, and unknown cleanup upgrading the whole result to indeterminate.
- Marker tests cover absent, empty file, arbitrary file, directory, valid symlink, Windows raw-32-then-success metadata retry, persistent metadata permission error, ambiguous entry, and broken symlink. The broken-link assertion is explicitly the inverted hard-error test described above.
- Exact formatter tests compare marker-present, marker-indeterminate, unmarked-indeterminate, cleanup/debris, retained-warning, and outbox strings byte-for-byte.

### Windows release subprocess acceptance

Extend the existing smoke using copies named `agentscommander_issue1577_<case-id>.exe` (the suffix is deliberately non-`dev`) and case roots under ignored `artifacts/cli-release-smoke`. Each GUI child receives `--app --test-home-dir <absolute-case-home>`; no `HOME`/`USERPROFILE` assumption is allowed. Launch every background child with `Start-Process -WindowStyle Hidden` and redirected streams; no case needs an interactive window. All environment changes are child-scoped. Resolve/capture the current user's ACL identity and the parent directory's original security descriptor before a case changes ACLs; restore that exact descriptor before scoped cleanup. Product code never participates.

For each adjacent-writability case, copy the binary first, ensure the exact adjacent candidate is absent, then add a deny rule limited to file/directory creation and write in the copied binary's parent while preserving read/execute and permission-restoration authority. A preflight child operation must observe raw Windows 5 for candidate creation; otherwise fail the fixture rather than accepting an ambiguous blocker.

1. **Unmarked primary fallback:** no `portable.txt`; remove child `AGENTSCOMMANDER_CONFIG_DIR` and `AGENTSCOMMANDER_TEST_CONFIG_DIR`; set child `AC_UI_AUTOMATION=0`; remove child `AC_TEST_WINDOW_PLACEMENT`; pass a fresh absolute case home through the hidden CLI input. Within 15 seconds require:
   - exact fallback config root `<child-home>/.agentscommander-new`;
   - exactly one `<fallback>/instances/<uuid>/outbox`, and `app-outbox-path.txt` plus the `[app-outbox]` stdout line both name that exact directory;
   - the adjacent candidate and marker remain absent and no adjacent state is created;
   - retained fallback diagnostics name the adjacent candidate, selected home, raw OS 5, and exhausted retries;
   - no fatal-selection text, `panicked at`, or backtrace hint.
   The healthy GUI child is expected to remain running; terminate it only after these assertions.
2. **Marker hard error:** create empty adjacent `portable.txt` before applying the same ACL blocker; clear both overrides and pass a fresh absolute test home. Require exit 1 within 15 seconds and the exact selection template containing candidate directory, terminal raw-5 OS reason, `AGENTSCOMMANDER_CONFIG_DIR`, and marker path. Require no test-home/outbox state and no panic text.
3. **Release public override:** retain the marker/blocked adjacent candidate, set `AGENTSCOMMANDER_CONFIG_DIR` to a distinct absolute blocking file path and `AGENTSCOMMANDER_TEST_CONFIG_DIR` to a third path, and pass a fourth fresh test home. Require exit 1 and the exact app-outbox template naming only the public override config/outbox and OS reason. Forbid marker/debug/adjacent/test-home selection messages and panic text. This proves the public variable exists in release and outranks every lower tier.

Every child has a 15-second observation/exit deadline. Before termination, snapshot the root plus recursively discovered descendant PID set. Invoke the resolved `$env:SystemRoot\System32\taskkill.exe /PID <root-pid> /T /F` once, bound that command itself to 10 seconds, then poll the captured identities (PID plus creation time, so PID reuse cannot satisfy the check) until all are terminal or the same deadline expires. Timeout, taskkill failure, surviving identity, ACL-restore failure, or artifact-cleanup conflict fails while preserving stdout/stderr/timing/PID/path/ACL diagnostics. Remove only case-owned artifacts after exact ACL restoration; retain diagnostic logs on failure.

This real release subprocess is mandatory. Unit composition without the unmarked `instance_location()`/outbox seam does not satisfy #1577.

### Required commands

From the repository root with locked dependencies and repository/CI toolchains:

```powershell
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib issue_1577_ -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib windows_settings_replace_retries -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test loops_layering
cargo test --manifest-path src-tauri/Cargo.toml --test instance_gitignore_layering
cargo test --manifest-path src-tauri/Cargo.toml --test project_settings_layering
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib --bins --tests
npm run build:prod:no-bundle
npm run smoke:cli-release-windows
```

Any old resolver/profile/settings assertion change, formatter drift, wrong retry count, unknown-error relocation, broken-link absence, probe residue without retained diagnostics, smoke timeout/tree leak, wrong selected path, panic text, or unexpected file change blocks delivery. Existing debt must be identified by exact established signature; no new failure is accepted as debt.

## Dependency-cycle and layering gate

Planned new/removed unique internal Rust module arcs: zero.

- `config::mod -> config::profile` already exists; replacing `config_dir_name` with the pure injected derivation preserves that pair.
- `config::settings -> config::mod` already exists at four production sites (`settings.rs:1775,2163,4191,4502`). Delegating to the shared retry helper adds a site, not a new `from -> to` pair.
- `lib -> config` and `main -> agentscommander_lib::{run,cli}` already exist; the added hidden CLI field, validator, `run` argument, and config initializer preserve those pairs.
- Profile/CLI extraction and probe/error work are intra-module; std/uuid are external.
- No lower layer gains CLI, Tauri, `AppHandle`, or UI transport. The config root owns shared config-filesystem policy; `lib/main` own presentation/exit.

Clean pinned-source `rust-levelization-run` evidence: Node `C:\Program Files\nodejs\node.exe` v24.13.0; detector exit 1 with graph emitted; 191 modules, 1,037 unique arcs/3,732 sites, 107 SCCs, exactly one cyclic SCC with 85 members. Regenerated `module-arcs.txt` was byte-identical to the tracked 82,149-byte/1,037-arc record, SHA-256 `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`.

Run on clean pinned-base and final-head worktrees:

```powershell
node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\Levelization\02-levelize.mjs" rank graph.json
node scripts/02-module-arc-record.mjs --graph graph.json --out src-tauri/module-arcs.txt
```

Detector exit 1 is normal when the existing cycle is measured and the graph exists; exit 3 blocks. Green requires:

1. `cyclicSccs` remains 1.
2. The 85-member cyclic SCC set is identical set-to-set.
3. Zero new unique `from -> to` pairs cross any previously clean SCC boundary; expected new/removed pairs are zero.
4. Regenerated `module-arcs.txt` is byte-identical and clean.
5. All three structural guards stay green.

A changed unique arc, SCC membership/count, role inversion, or arc-record byte drift returns to architecture review. A higher edge-site count on a pre-existing pair is expected and is not a new module arc.

## Delivery and nonfunctional gates

### CI parity and deterministic tools

The pinned workflows make these PR jobs applicable: `test-debt`; Windows `rust-regression`; Linux/macOS Rust regression; the four-host terminal-snapshot matrix; `windows-release-cli-smoke`; `frontend-regression`; and `lockfile-drift`, whose detector must report no package-manifest change. Bundle/version path filters do not match. Re-derive only if relevant workflow/config/diff paths drift.

CI uses Node 22, npm 11.6.2, Rust stable, `npm ci`, and committed Cargo lock resolution. Local runs record resolved versions. Host-dependent release/ACL behavior belongs to Windows CI. Every triggered and configured-required check must pass on the exact PR-head SHA; another SHA, unexplained skip, bypass, or waiver is insufficient.

### Git, bounded drift, mutation, and recovery

- The immutable planning base remains `1eee2cd...` by release-owner ruling. Do not add a rebase prerequisite merely because `main` moved.
- Immediately before first product mutation and PR creation/update, fetch live `main` and classify drift from the pinned base by affected source/test/workflow/toolchain semantics. Refresh only materially affected evidence; unrelated motion cannot restart accepted design.
- Require the issue branch, recorded base/head, clean index, no unrelated tracked/untracked state, and frozen seven-path scope. Product Git writes occur only in authorized `repo-AgentsCommander` and deliver through a PR, never direct to `main`.
- Before each write, recheck affected bytes. Recovery may unstage/remove only this run's newly created or demonstrably unchanged output. Never use broad reset/restore/clean.
- Smoke/build artifacts stay in repository-standard ignored roots. ACL recovery restores the captured case-root descriptor before deleting only owned artifacts; a mismatch preserves evidence and fails.
- Final evidence enumerates tracked, staged, ordinary-untracked, lockfile, config, workflow, and intended path state.
- Hash plan certifications from `git cat-file blob <commit>:plans/1577-writable-config-resolver.md`, not worktree bytes or `git show`.

### Bounded diagnostics and enhanced controls

Tests/builds use runner/CI timeouts and retain exit/stdout/stderr. The release child uses explicit 15-second observation plus 10-second tree-cleanup bounds; timeout/cancellation is failure and cleanup cannot erase the primary failure.

No enhanced signing, binary-hash, helper inventory, hostile-parent quarantine, product ACL mutation, reparse proof, custom transaction ledger, or exclusive cross-process lock applies. The ACL fixture is scoped test setup for the issue's primary Windows hazard; shared retries, exact-head CI, real subprocess acceptance, scoped diff, and no-clobber cleanup are proportionate.

## Ready/acceptance verdict

Implementation is accepted only when the precedence/cache contract, one-capture pure derivation, unchanged first-underscore/profile behavior, hidden validated test-home input, shared #1436 retry schedule, closed conclusive allowlist, marker-indeterminate hard error, inverted broken-link assertion, cleanup/debris diagnostic, exact messages, exit 1, nine legacy outcomes, seven-path scope, mandatory unmarked release subprocess, dependency criterion, local checks, and exact-head CI all pass.

Any request to silently relocate on an unknown error, coerce marker ambiguity to absent, duplicate the retry policy, change suffix parsing, add #1137/#1594/AppImage behavior, or weaken process-tree/ACL recovery is a blocker back to the release owner.
