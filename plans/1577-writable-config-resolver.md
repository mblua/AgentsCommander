# #1577 — Writability-aware config resolver with portable marker

Status: READY_FOR_IMPLEMENTATION

## Delivery identity

- Issue: open #1577, approved specification comment #issuecomment-5444596303, and the release-owner round-two and round-three rulings.
- Delivery path: Full.
- Target: main pinned at 1eee2cd72a0d25095108d92b3f495da84b979d24.
- Branch: fix/1577-writable-config-resolver, created from that exact target.
- The pinned base stands by release-owner ruling. Later target movement is handled by bounded drift classification; it is not a rebase gate.
- Planned application/test paths, and no others:
  - src-tauri/src/config/mod.rs
  - src-tauri/src/config/profile.rs
  - src-tauri/src/config/settings.rs
  - src-tauri/src/lib.rs
  - src-tauri/src/main.rs
  - scripts/smoke-cli-release-windows.ps1
- Plan artifact: plans/1577-writable-config-resolver.md.
- Round three returns to the authorized six-file implementation set. Rejecting the shipped test-home seam removes src-tauri/src/cli/mod.rs. The existing Windows smoke wrapper remains only to prove the separate release-build public-override binding through its already-existing CLI invocations; it gains no ACL, marker/probe, copied-GUI, or adjacent-writability case.
- The separate release-owner workflow prerequisite is a change to .github/workflows/pr-regression-gates.yml. It is explicitly outside this implementation branch and is specified under “Linux CI prerequisite.”
- Task class: routine application/test work with elevated state-location safety impact. The accepted threat model covers accidental state relocation, a real unprivileged-Linux permission failure, short-lived Windows antivirus/file-sharing interference, ambiguous marker metadata, probe debris, and startup failure visibility. It does not include hostile-host/toolchain provenance, malicious PATH, signing/packaging provenance, destructive migration, or an untrusted build host.

## Objective and verified cause

Make an unmarked executable-adjacent config location conditional on a successful real write probe, add the supported release-build override, make portable.txt a hard no-fallback assertion, and replace the app-outbox panic with a visible exit-1 startup error.

Evidence at the pinned source:

- src-tauri/src/config/mod.rs:87-155, resolve_instance_location, always selects <exe_parent>/.<exe_stem> when the executable has a parent/stem. It never checks writability.
- src-tauri/src/config/mod.rs:158-168, instance_location, owns the process-lifetime OnceLock and performs the resolver’s explicit current_exe capture.
- The home branch calls profile::config_dir_name. At profile.rs:45-56 that calls binary_suffix, whose separate OnceLock at profile.rs:20-30 performs another current_exe call. A resolver that merely injects current_exe_result is therefore not transitively deterministic.
- binary_suffix splits the executable stem at the first underscore; config name, title, mutex, instance label, and both port derivations depend on that behavior. It must not change.
- src-tauri/src/config/settings.rs:4413-4452 implements #1436: Windows raw OS 5 and 32 retry with delays [15, 30, 60, 120, 240] before a final sixth attempt. The config probe must share this policy.
- src-tauri/src/lib.rs:1823-1870 obtains config_dir and panics when creating <config>/instances/<uuid>/outbox.
- src-tauri/src/main.rs:5-152 already owns GUI process exit and calls both agentscommander_lib::run and cli::present_fatal_startup_message.
- The nine resolver tests at config/mod.rs:208-359 pin every existing override, executable, home, stem, and absolute-only instance-base result.
- The reported primary failure is Ubuntu with a non-root launch from a root-owned global npm prefix. Locked resolution is dirs 6.0.0. On Linux its home_dir follows HOME, so a child-only HOME value controls the real fallback input without product injection; on Windows it uses FOLDERID_Profile and is deliberately not used for this acceptance seam.

## Scope and exclusions

In scope:

- The precedence, marker states, retry/classification policy, cleanup semantics, and cache lifetime below on Windows, macOS, and Linux.
- A deterministic profile derivation over the resolver’s one captured executable without changing first-underscore parsing or downstream title/mutex/port behavior.
- One shared Windows retry policy used by settings replacement and the config probe.
- Typed cached startup failure and fallback diagnostics without changing existing config projections.
- A typed lib::run result, fatal presentation in main, focused unit tests, and one exactly named Linux self-subprocess test over the real instance_location and app-outbox creation seam.
- Release-build evidence for AGENTSCOMMANDER_CONFIG_DIR by running the existing Windows CLI smoke invocations under a scoped public override and asserting their app.log location.

Out of scope:

- Any --test-home-dir flag, hidden CLI field, CLI validator, main-to-run test-home argument, public/general home override, shipped test affordance, or src-tauri/src/cli/mod.rs change.
- Any Windows ACL, marker/probe, copied-GUI, home-fallback, or adjacent-writability end-to-end case. Windows raw-5/raw-32 composition is covered only by injected unit tests and settings.rs’s existing tests. The existing release CLI smoke may verify only the independent public-override binding described below.
- #1137’s Linux package classifier, XDG target/input/error, general fallible config projection, and reserved precedence slot 3.5.
- #1594’s macOS bundle-container target and any genuine portable AppImage rule.
- Packaging, npm, manifest, lockfile, frontend, schema, migration, documentation, and unrelated startup-expect changes.
- Canonicalization, permission-bit inspection in product code, product ACL mutation, UAC configuration, symlink/reparse hardening beyond the marker-state decision below, and a second fatal-message surface.
- The existing config_dir().expect("Cannot determine home directory") when both executable and home inputs are unavailable. Tier 6 preserves that existing None result.

The release owner owns the documentation claim delta: an unmarked copy is adjacent only while the real probe succeeds; portable.txt forbids fallback. Documentation and packaging remain release-track owned.

## Required resolution contract

std::env::var supplies both overrides. Non-Unicode values remain unset. A Unicode value is unset only when raw.trim().is_empty(); a selected path uses the original untrimmed string verbatim.

First decisive state wins:

1. Nonblank AGENTSCOMMANDER_CONFIG_DIR in debug and release selects PathBuf::from(raw) verbatim. Absolute paths expose their parent as instance_base; relative paths expose no base. The executable still determines local_dir_stem.
2. Nonblank AGENTSCOMMANDER_TEST_CONFIG_DIR, compiled/read only under debug_assertions, has the same path/base/stem semantics. The public override always wins.
3. With no override and a usable executable, inspect <exe_parent>/portable.txt:
   - indeterminate marker state is a hard startup error; do not probe the candidate or consider home;
   - marker present plus write success selects <exe_parent>/.<exe_stem>;
   - marker present plus any exhausted write-probe failure is a hard startup error and never considers home.
4. Marker absent plus write success selects the executable-adjacent candidate, retaining today’s relative-path and absolute-only-base behavior.
5. Marker absent plus a conclusively-unwritable exhausted probe selects $HOME/<profile config name derived from the same captured executable>, with no base. current_exe failure or a path without parent/stem also reaches this home tier without marker/write I/O.
6. If the applicable home fallback has no home directory, preserve config_dir: None and instance_base: None.

Marker absent plus an indeterminate exhausted probe is not tier 5: it is a hard startup error with no relocation. Slot 3.5 remains reserved for #1137.

Both override branches short-circuit marker and write I/O. Invalid executable shapes skip both probes. The process never re-probes or changes root after instance_location initializes.

## One executable capture and transitive purity

The resolver has exactly seven explicit inputs:

    pub(crate) fn resolve_instance_location(
        public_override: Option<String>,
        test_override: Option<String>,
        current_exe_result: Result<PathBuf, std::io::Error>,
        home_dir: Option<PathBuf>,
        fallback_config_dir_name: &str,
        marker_probe: MarkerProbeOutcome,
        write_probe: WriteProbeOutcome,
    ) -> InstanceLocation

instance_location captures its public override, debug override, current_exe result, and dirs::home_dir result once. From that same captured executable result it derives fallback_config_dir_name before calling the resolver. No test-home input or alternate initializer exists.

Its production adapter uses this exact short-circuit sequence, without callbacks inside the resolver:

1. If the public override is nonblank, the debug override is the effective nonblank fallback, or the captured executable lacks a usable parent/stem, supply MarkerProbeOutcome::NotRun and WriteProbeOutcome::NotRun.
2. Otherwise probe the marker once through the retry wrapper.
3. For an Indeterminate marker supply WriteProbeOutcome::NotRun. For Present or Absent, run the real write probe against the captured executable-adjacent candidate.
4. Call the pure resolver exactly once with the captured values and final outcomes.

This small orchestration duplicates no path-selection result: the resolver remains the sole owner of which InstanceLocation/error the inputs select, while the adapter owns only whether the next I/O operation is permitted.

In profile.rs:

- Extract a pure executable-path-to-suffix helper from binary_suffix. Preserve exactly: file stem through to_string_lossy, first find('_'), and every byte after that first underscore. Empty, multiple-underscore, and no-underscore behavior remains unchanged.
- Keep binary_suffix, its OnceLock, all existing callers, and every title/mutex/instance-label/port outcome. It delegates parsing to the pure helper but retains capture/cache behavior.
- Add a pub(super) pure config-name derivation accepting Option<&Path>. It applies the same suffix parser and existing BUILD_PROFILE rule and returns the same two static names.
- Make existing config_dir_name delegate name selection through the same pure rule, retaining its signature, cache, and behavior.

resolve_instance_location joins home_dir with the injected fallback_config_dir_name and never calls either profile accessor. Its transitive call graph contains no environment read, filesystem operation, OnceLock mutation, UUID/time/sleep, logging, or callback. Identical seven inputs yield identical fields, errors, and diagnostics.

Production derives the injected name through the pure profile helper over current_exe_result.as_ref().ok(). Every legacy test passes its existing profile::config_dir_name result as the seventh purity input, so all nine assertions stay unchanged while only call signatures grow.

## Marker probe: present, absent, or indeterminate

Do not use Path::exists. The production marker adapter runs each symlink_metadata/follow-up metadata call through the shared retry helper, then returns:

- Absent only when the marker entry itself returns NotFound.
- Present for a regular file or directory.
- For a symlink, a resolvable regular-file/directory target is Present.
- A broken link, loop, unreadable target, unsupported entry/target type, or any other ambiguous result is Indeterminate.
- Any metadata error other than entry-level NotFound is Indeterminate after applicable retries and retains operation, path, attempts, kind, raw OS code, and OS reason.
- NotRun is valid only for higher overrides or unusable executable shapes.

Marker contents are never opened. Native case semantics apply. Marker indeterminate uses the same typed hard-error surface as marker-present probe failure, includes the marker path, and never enters the unmarked branch.

Marker-metadata reasons are exact:

- could not inspect portable marker entry metadata "{affected_path}" after {attempts} attempt(s): {os_reason}
- could not resolve portable marker symlink target metadata "{affected_path}" after {attempts} attempt(s): {os_reason}
- filesystem metadata reported an unsupported portable marker entry type

The round-one assertion “a broken marker link is absent” is explicitly inverted. The exact test broken_marker_link_is_indeterminate_and_never_falls_home asserts a hard error, no write-probe consumption, and no home selection. It must not be retained with the old outcome, renamed to hide the inversion, or deleted.

## Shared retry policy and final classification

Put one test-injectable transient-I/O retry policy in config/mod.rs and make both the config probe and settings.rs’s existing atomic-replace wrapper delegate to it.

- Move/generalize the sole Windows backoff constant to the config root: exactly [15, 30, 60, 120, 240] ms.
- On Windows, raw OS 5 (ERROR_ACCESS_DENIED) and 32 (ERROR_SHARING_VIOLATION) receive the five existing sleeps and a final sixth attempt. Existing #1436 call/sleep/error behavior and tests remain unchanged.
- ErrorKind::Interrupted receives one immediate additional attempt on every platform. It consumes no sleep. The budget is per required filesystem operation and cannot reset recursively.
- On non-Windows, every non-Interrupted error returns after the first attempt. PermissionDenied and ReadOnlyFilesystem never receive Windows-shaped sleep/backoff.
- A chain mixing Interrupted with Windows raw 5/32 has at most seven calls: initial, one immediate interrupted retry, and five Windows delayed retries. The returned error is the terminal exhausted error.

Only after retries exhaust may the probe classify the error. The conclusive allowlist is closed:

- Windows raw OS 5 or 32; or
- ErrorKind::PermissionDenied or ErrorKind::ReadOnlyFilesystem on any platform.

Everything else is Indeterminate, including persistent Interrupted, unknown raw OS errors, AlreadyExists, NotFound during the required delete, path-shape errors, storage-full errors, and any future kind not explicitly added. Unknown means hard startup error, never silent relocation.

The Windows raw-5/raw-32 wiring is tested only through injected error sequences. There is no ACL setup, copied Windows GUI executable, hidden input, or Windows adjacent-writability end-to-end #1577 case.

The accepted residual gap is explicit: this delivery does not compose a real Windows ACL denial with the full process boundary. The closed classifier is unit-testable on Windows, the resolver/startup wiring is platform-independent, and the reported Linux composition is exercised for real. The release owner accepted losing only that Windows-specific composition.

## Real write probe and cleanup/debris contract

For candidate directory D:

1. Retry-wrapped create_dir_all(D).
2. Retry-wrapped create_new of D/.agentscommander-write-probe-<Uuid::new_v4()>.tmp with write enabled and no truncate/append.
3. Retry-wrapped write_all of the exact bytes AgentsCommander write probe followed by newline, on that owned handle.
4. Close the handle.
5. Retry-wrapped remove_file of that exact probe file.

uuid v4 is already a production dependency. No sync_all, permission-bit decision, recursive delete, candidate rollback, or new dependency is allowed. A newly created successful candidate remains; the probe file does not.

Cleanup rules:

- No cleanup after create_new fails because this invocation did not establish file ownership.
- After a post-create write failure, close the handle and run the same retry helper around best-effort removal of only the owned probe file.
- NotFound during best-effort cleanup proves no debris and is cleanup success. NotFound during required step 5 remains an indeterminate probe failure.
- Preserve terminal primary operation/path/attempts/kind/raw-code/OS-reason. If cleanup exhausts, preserve its complete terminal reason and set probe_may_remain: true with the exact probe path.
- An exhausted required step-5 delete also sets probe_may_remain: true. Do not start another removal loop after the shared helper consumed its budget.
- Aggregate classification is conclusive only when every retained non-success error is in the closed allowlist. Any indeterminate primary or cleanup error makes the outcome indeterminate.
- Successful cleanup removes transient cleanup errors from the final outcome. Exhausted cleanup is never discarded.

For marker absent plus a conclusive failure, cache AdjacentFallbackDiagnostic alongside the home selection. It includes candidate, selected home when any, complete primary/cleanup reasons, probe path, and debris flag. After logger initialization, lib::run emits one warning from that retained diagnostic. Marker-present failures and unmarked indeterminate failures return before logging/state initialization.

## Cached data, typed errors, and process boundary

In config/mod.rs:

- Add cloneable marker/probe failure data, ProbeFailureClass, and ConfigStartupError::AdjacentSelectionBlocked { config_dir, marker_path: Option<PathBuf>, reason }.
- Extend InstanceLocation privately with startup_error: Option<ConfigStartupError> and fallback_diagnostic: Option<AdjacentFallbackDiagnostic>.
- Keep the single no-argument instance_location OnceLock orchestration. Do not add initialize_instance_location, test-home input, or reset hook.
- Keep config_dir() -> Option<PathBuf>, instance_base() -> Option<PathBuf>, and agent_local_dir_name() -> String unchanged.
- Add only crate-private clone projections for startup_error and fallback_diagnostic.

Every hard adjacent-selection result keeps config_dir: Some(candidate), the existing local_dir_stem, and the same absolute-only executable-parent instance_base that a successful adjacent selection would expose, while also setting startup_error. It never substitutes home and never returns config_dir: None. This lets the typed error name the resolved directory without changing projection signatures; lib::run must check startup_error before consuming those projections for state creation.

Exact startup text without a known marker:

    AgentsCommander cannot start because configuration directory "{config_dir}" could not be safely selected: {reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.

When marker status is present or indeterminate, append:

    Portable marker path: "{marker_path}".

Every hard selection message therefore contains directory, terminal filesystem/OS reason, and AGENTSCOMMANDER_CONFIG_DIR; marker-present/indeterminate states also contain the marker path. Paths use display formatting. Kind/raw code remain in typed diagnostics/tests.

Write-probe reasons are exact:

- write probe could not create configuration directory "{affected_path}" after {attempts} attempt(s): {os_reason}
- write probe could not create probe file "{affected_path}" after {attempts} attempt(s): {os_reason}
- write probe could not write probe file "{affected_path}" after {attempts} attempt(s): {os_reason}
- write probe could not delete probe file "{affected_path}" after {attempts} attempt(s): {os_reason}

An exhausted post-failure cleanup appends:

    Cleanup of probe file "{probe_path}" also failed after {attempts} attempt(s): {cleanup_os_reason}; the probe file may remain.

An exhausted required delete appends:

    The probe file "{probe_path}" may remain.

In lib.rs:

- Add a public opaque StartupError struct containing a private StartupErrorKind enum. The private enum variants are Config(ConfigStartupError) and AppOutboxCreate { config_dir, app_outbox_path, source: io::Error }. Implement Display and Error on the public wrapper; main only needs to_string, while lib.rs unit tests can inspect the private kind.
- Change run(test_window_placement, ui_automation_enabled) from () to Result<(), StartupError>. Do not change its arguments.
- Before logger/token/settings/state mutation, read and return the cached config startup error.
- Initialize the logger, then emit the retained conclusive-fallback warning.
- Extract the existing outbox path calculation/create_dir_all into a private prepare_app_outbox(config_dir: &Path, instance_id: &str) -> Result<(PathBuf, AppOutbox), StartupError>. Production run and the Linux child test call this same function. It is not public, CLI-addressable, or compiled as a separate test seam.
- Replace only the current app-outbox expect through that helper; preserve creation order and return Ok(()) after the Tauri run loop.

Exact app-outbox text:

    AgentsCommander cannot start because it could not create app outbox directory "{app_outbox_path}" for configuration directory "{config_dir}": {os_reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.

In main.rs:

- Keep CLI parsing and run’s two arguments unchanged.
- Handle run’s Result. On Err, call agentscommander_lib::cli::present_fatal_startup_message(&error.to_string()) exactly once and exit 1.
- Preserve every other parse, CLI-command, UI-automation, mutex/second-instance, and exit behavior.

Neither startup error contains panic/backtrace text.

## Platform and compatibility behavior

- Windows, macOS, and Linux share marker/probe/classification semantics. Only the retry schedule is platform-specific as stated.
- A short raw-32/raw-5 Windows hold that clears within #1436’s schedule stays adjacent. Persistent raw 5/32 is conclusively unwritable and reaches home when unmarked; this composition is unit-tested with injected errors only.
- Unix EACCES/read-only results classify immediately without sleep. Unknown Unix errors hard-stop.
- Writable zip/USB/user-folder copies remain adjacent, including renamed executable isolation. Root/system installs fall home when unmarked. A marker converts any inability/ambiguity into visible refusal.
- macOS bundle and Linux AppImage behavior remain safety nets, not #1594/#1137 placement implementations.
- Existing schemas, contents, stem, mutex, labels, ports, absolute-only base, and verbatim relative override behavior remain unchanged.
- Product code never changes ACLs or modes. create_new plus UUID prevents truncation/clobber. Cleanup is limited to the file this process created.
- config gains no CLI, Tauri, AppHandle, or UI transport dependency. Presentation and process termination stay at lib/main.

## Affected files and symbols

1. src-tauri/src/config/mod.rs
   - Shared retry policy, marker/probe outcomes and adapters, classification, cleanup, typed errors/diagnostics.
   - Seven-input pure resolver and one-shot instance_location orchestration.
   - Unchanged public projections plus narrow startup/diagnostic projections.
   - Existing nine tests retained; focused #1577 tests added.
2. src-tauri/src/config/profile.rs
   - Pure suffix/config-name derivation; existing binary_suffix/config_dir_name delegate without changing first-underscore behavior.
3. src-tauri/src/config/settings.rs
   - Existing Windows atomic-replace wrapper delegates to the shared retry policy; #1436 behavior and its own tests stay intact.
4. src-tauri/src/lib.rs
   - StartupError, early config failure, retained fallback warning, private shared app-outbox preparation, fallible run, and the Linux self-subprocess test inside the existing tests module.
5. src-tauri/src/main.rs
   - GUI Result handling, existing fatal presentation, and exit 1.
6. scripts/smoke-cli-release-windows.ps1
   - Scoped environment guard around the existing release CLI smoke invocations and post-run assertions that the public override, not the debug canary, owns app.log.

No module/file move, new source/test file, dependency, lockfile, frontend, IPC, persistence schema, workflow, packaging, npm, documentation, CLI module, or Windows ACL/GUI smoke addition is allowed in the implementation PR.

## Ordered implementation

1. Recheck branch/base/index/affected bytes and freeze the six-path scope. Stop on unrelated dirty state.
2. Split profile capture from derivation and pin first-underscore behavior before changing resolver calls.
3. Generalize the #1436 retry policy in the config root; delegate settings replacement and prove its old tests unchanged.
4. Add marker tri-state, write probe, final classification, cleanup/debris data, and retained diagnostic.
5. Extend InstanceLocation and the pure resolver; preserve all nine old results and implement the decisive-state table.
6. Make instance_location perform the one resolver capture, short-circuit I/O, run real adapters once, and cache the result.
7. Make lib::run fallible, extract private app-outbox preparation, and let main present/exit without changing CLI shape.
8. Add focused unit tests, the exactly named Linux self-subprocess test, and the scoped release public-override assertions around existing Windows smoke invocations.
9. Format, validate locally, rerun dependency/scope gates, and deliver through the issue PR with exact-head CI.

## Tests and objective acceptance

### Pure resolver/profile tests

- Prefix every new #1577 unit test with issue_1577_ except the release-owner-required broken-link identifier, which remains broken_marker_link_is_indeterminate_and_never_falls_home.
- Retain all nine named resolver tests and every assertion; calls gain public override, profile::config_dir_name as the injected fallback name, and marker/write outcomes.
- Existing adjacent rows inject marker absent/write success. Existing fallback rows retain exact path/stem/base results.
- Public override beats simultaneous debug override, marker, and failed outcomes. Blank values fall through correctly.
- Profile tests pin no underscore, first underscore, multiple underscores, empty suffix, dev suffix, and current build-profile fallback. Existing mutex/title/port tests stay unchanged.
- Resolver tests prove marker present success, marker present conclusive/indeterminate failure, marker absent success, marker absent conclusive home fallback, marker absent indeterminate hard error, invalid executable short-circuit, and no-home tier 6.
- Identical seven inputs produce identical complete InstanceLocation data; no resolver call installs callbacks or reaches process-global profile caches.

### Retry, classification, marker, and cleanup tests

- Existing windows_settings_replace_retries_access_denied and windows_settings_replace_retries_sharing_violation retain their two-call/[15] assertions in settings.rs.
- Shared-helper tests pin six persistent raw-5/raw-32 calls and sleeps [15,30,60,120,240], transient success, one immediate Interrupted retry on every platform, the seven-call mixed upper bound, and no Unix sleep for permission/read-only errors.
- The Windows replacement for the discarded ACL composition is the #[cfg(windows)] injected unit test issue_1577_windows_raw_5_exhaustion_is_conclusive_without_real_acl. It feeds io::Error::from_raw_os_error(5), asserts six calls, the exact five sleeps, terminal conclusive classification, and unmarked home selection. A parallel raw-32 row uses the same table; neither touches a real ACL or launches a child.
- Classification table tests prove only the closed allowlist is conclusive and an unknown raw code/future-other kind is indeterminate.
- Real successful probes cover existing/missing directories, retained created directory, and zero probe residue.
- A real non-directory candidate is indeterminate/hard and must not masquerade as permission failure.
- Injected cleanup tests cover interrupted-then-success, Windows sharing-violation-then-success, persistent conclusive delete with retained debris diagnostic, and unknown cleanup upgrading the outcome to indeterminate.
- Marker tests cover absent, empty file, arbitrary file, directory, valid symlink, Windows raw-32-then-success metadata retry, persistent metadata permission error, ambiguous entry, and broken symlink.
- Exact formatter tests compare marker-present, marker-indeterminate, unmarked-indeterminate, cleanup/debris, retained-warning, and outbox strings byte-for-byte.
- Windows raw-5/raw-32 behavior is tested only through injected error kinds. No Windows ACL or adjacent-writability end-to-end #1577 case is permitted; the existing release smoke covers only the independent public override.

### Existing Windows release smoke: public override only

scripts/smoke-cli-release-windows.ps1 keeps its existing release binaries, shells, child script, CLI cases, logs, and pass/fail aggregation. It adds no new GUI/background process case and never exercises the adjacent marker/probe/classifier composition.

For each existing binary/shell case:

1. Choose fresh absolute paths under that case root for public-config and debug-canary, derive the executable-adjacent .<binary-stem> candidate, and require all three absent before invocation.
2. Capture whether AGENTSCOMMANDER_CONFIG_DIR and AGENTSCOMMANDER_TEST_CONFIG_DIR existed in the wrapper environment and their exact prior values.
3. In a try/finally scoped only around the existing smoke-cli-powershell.ps1 invocation, set AGENTSCOMMANDER_CONFIG_DIR to public-config and AGENTSCOMMANDER_TEST_CONFIG_DIR to debug-canary. Restore exact presence/value in finally, including on child failure.
4. After the existing CLI cases return, require <public-config>/app.log to exist. logging::init_logger in the existing command path writes to <config_dir>/app.log, so this is the observed release-binary root.
5. Require debug-canary and the executable-adjacent config candidate to remain absent. Record the two canary paths and assertions in the existing case/summary logs.

This is not the rejected Windows raw-5 case: it changes no ACL, does not control home, does not place a marker, does not launch the GUI, and does not claim Windows classifier-to-process composition. It proves only that the supported public variable is read by a release binary; the pure resolver unit separately proves public-over-debug precedence.

### Exact Linux subprocess acceptance

The exact test identifier is:

    tests::issue_1577_linux_unmarked_adjacent_unwritable_falls_back_to_home

It is a #[cfg(target_os = "linux")] test inside lib.rs’s existing tests module. Existing dev-dependency tempfile = "3" supplies its two fixture roots, so Cargo.toml/Cargo.lock do not change. It has parent and child branches selected only by the test-compiled child sentinel AGENTSCOMMANDER_ISSUE_1577_CHILD; no product code reads that name.

Parent fixture:

1. Create separate tempfile case-root and home TempDirs. Do not mutate the parent process environment or instance_location.
2. Copy std::env::current_exe() to <case-root>/agentscommander_issue1577_linux_subprocess and preserve/set executable mode.
3. Require portable.txt and .agentscommander_issue1577_linux_subprocess to be absent.
4. Change only case-root to mode 0555. A same-user create_new preflight inside case-root must fail with ErrorKind::PermissionDenied; an unexpected success is removed after mode restoration and fails the fixture. This rejects root or a filesystem that ignores the mode instead of producing a false pass.
5. Launch only the copied test executable with the exact identifier, --exact, --nocapture, and --test-threads=1. Its child environment sets HOME to the separate writable home, removes AGENTSCOMMANDER_CONFIG_DIR and AGENTSCOMMANDER_TEST_CONFIG_DIR, and sets only the test-compiled child sentinel.

Child behavior:

1. Call crate::config::config_dir(), thereby executing the real no-argument instance_location OnceLock, marker adapter, write probe, classification, and dirs::home_dir path.
2. Assert the observed config root is exactly <HOME>/.agentscommander-new. The copied stem is deliberately non-dev.
3. Call the same private prepare_app_outbox used by run with fixed instance id 00000000-0000-4000-8000-000000001577.
4. Assert the returned/created outbox is exactly <HOME>/.agentscommander-new/instances/00000000-0000-4000-8000-000000001577/outbox.
5. Assert the retained fallback diagnostic names the adjacent candidate, selected home, one-attempt Unix PermissionDenied create-directory failure, and no probe debris.

Parent assertions after child exit:

- Exit status is 0; stdout/stderr contain no “panicked at” or backtrace hint.
- The exact home config and fixed outbox directory exist, with exactly that single instance subtree.
- The adjacent candidate remains absent, portable.txt remains absent, and case-root contains only the copied test executable. No adjacent config, marker, probe file, token, log, or outbox state exists.
- The fixture restores case-root’s original mode before cleanup and removes only its two TempDirs.

The child has a 15-second completion deadline. On timeout the parent kills only that known child, polls/reaps it for at most 5 more seconds, restores the exact original directory mode in all paths, and fails with retained stdout/stderr/status/fixture paths. The child branch spawns no descendants. After terminal child state and mode restoration, each TempDir receives one explicit close attempt; there is no retry loop. A mode-restore, child-reap, or scoped TempDir cleanup failure is test failure and retains the affected path for diagnosis.

This proves the reported Ubuntu composition without a CLI injection point: real current_exe identity, real absent marker, real create_dir_all PermissionDenied, real HOME fallback, the cached instance_location projection, and the same app-outbox creator used by run.

### Required commands

From the repository root with locked dependencies and repository/CI toolchains:

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

On an unprivileged Linux host, additionally run exactly:

    cargo test --manifest-path src-tauri/Cargo.toml --lib 'tests::issue_1577_linux_unmarked_adjacent_unwritable_falls_back_to_home' -- --exact --test-threads=1 --nocapture

Existing workflow-owned npm production build, frontend, terminal-snapshot, and lockfile gates remain applicable. The Windows release CLI smoke is edited only for the scoped public-override assertions above; its existing cases and child script remain unchanged.

Any old resolver/profile/settings assertion change, formatter drift, wrong retry count, unknown-error relocation, broken-link absence, probe residue without retained diagnostics, missing public-override app.log, created debug/adjacent canary, environment-restore failure, Linux subprocess timeout/cleanup defect, wrong selected path, panic text, zero-test filter result, or unexpected file change blocks delivery. Existing debt must be identified by exact established signature; no new failure is accepted as debt.

## Linux CI prerequisite and local substitute evidence

Current authoritative workflow state:

- .github/workflows/pr-regression-gates.yml rust-regression runs on windows-latest and executes cargo test --lib --bins --tests.
- rust-regression-linux runs on ubuntu-latest but executes only cargo check --all-targets and cargo clippy --all-targets -- -D warnings. It runs no tests.

Therefore the acceptance statement “the named Linux subprocess test passes in CI” is not satisfiable on the current workflow. Merely committing a Linux-only test would compile it through check/clippy and execute it nowhere. This plan does not claim current Linux CI coverage.

External prerequisite, named and owned:

- Name: release-owner #1577 exact Linux test step.
- Owner: release owner.
- File/job: .github/workflows/pr-regression-gates.yml, rust-regression-linux.
- Timing: land on main before the #1577 product PR’s merge-eligible exact-head run.
- Exact command from src-tauri:

      cargo test --lib 'tests::issue_1577_linux_unmarked_adjacent_unwritable_falls_back_to_home' -- --exact --test-threads=1 --nocapture

- It must not run the Linux suite or a broad issue_1577_ filter.
- On the #1577 PR head, the step is green only when its log names that exact test, reports “1 passed; 0 failed,” reports zero ignored/measured, and does not report zero tests. A zero-match cargo exit is an evidence failure even if Cargo returns 0.

Until that prerequisite is live, the implementation owner supplies the only execution evidence for this delivery:

- Run on an ordinary non-root Ubuntu 24.04 x86_64 host, or equivalent unprivileged Linux host, backed by a filesystem that enforces Unix mode bits.
- Record uname -a, id -u (must be nonzero), rustc -Vv, cargo -V, git rev-parse HEAD, clean git status, the exact command above, its complete stdout/stderr, and exit code.
- Proof is exit 0 plus the exact harness line “test tests::issue_1577_linux_unmarked_adjacent_unwritable_falls_back_to_home ... ok” and final “1 passed; 0 failed”; fixture preflight or cleanup failure invalidates the run.
- Run at the final candidate head before review. If no qualifying Linux host is available or the exact output is absent, implementation review blocks rather than calling the primary case covered.

After the workflow prerequisite lands, local evidence remains complementary. The exact PR-head Linux CI step becomes authoritative for merge; another SHA, a skipped step, a zero-match log, or an unexplained waiver does not satisfy the gate.

## Dependency-cycle and layering gate

Planned new/removed unique production Rust module pairs: zero.

- config/mod.rs continues to reference its existing profile child; replacing config_dir_name with the pure path-derived helper changes the item, not the source/target module relationship.
- config::settings -> config already exists at settings.rs:1775, 2163, 4191, and 4502. Delegating to the shared retry helper adds a site on that pair.
- lib -> config already exists. Reading startup diagnostics and calling existing config projections adds sites only.
- main’s binary-to-library calls to run and cli already exist; Cargo crate dependencies cannot be cyclic. No new crate dependency is added.
- The Linux test is inline under lib.rs’s existing #[cfg(test)] tests module, adds no source module/file, and follows the already-existing lib -> config direction. Test-only code is excluded from the production detector by contract.
- Profile extraction, probe/error logic, and private app-outbox extraction are intra-module.
- No lower layer gains CLI, Tauri, AppHandle, or UI transport. Config owns filesystem selection; lib/main own presentation and termination.

Fresh clean pinned-source rust-levelization-run evidence:

- Resolved Node: C:\Program Files\nodejs\node.exe, v24.13.0.
- Detector exit 1 with a valid graph: 191 modules, 1,037 unique arcs, 3,732 sites, 107 SCCs, exactly one cyclic SCC with 85 members.
- Sorted LF/no-tail cyclic-member-set SHA-256: CCDD957D7BDA9F7C2D164C3216F17310285EF7B80D52C6CBABBAFD495D73E244.
- Graph SHA-256: 915395E0E89BC793E04FFFC2981BF1C15A50A248BD62BBBF0636B2492F90E693.
- Regenerated module-arcs.txt is byte-identical to the tracked 82,149-byte/1,037-arc record, SHA-256 A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E.

Run on clean pinned-base and final-head worktrees:

    node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
    node "..\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\Levelization\02-levelize.mjs" rank graph.json
    node scripts/02-module-arc-record.mjs --graph graph.json --out src-tauri/module-arcs.txt

Detector exit 1 is normal when the existing cycle is measured and the graph exists; exit 3 blocks. Green requires:

1. cyclicSccs remains 1.
2. The 85-member cyclic SCC set is identical set-to-set.
3. Zero new unique from -> to pairs cross a previously clean SCC boundary; expected new/removed pairs are zero.
4. Regenerated module-arcs.txt is byte-identical and clean.
5. loops_layering, instance_gitignore_layering, and project_settings_layering stay green.

A changed unique arc, SCC membership/count, role inversion, or arc-record byte drift returns to architecture review. An added site on a pre-existing pair is not a new module arc.

## Delivery and nonfunctional gates

### CI parity and deterministic tools

The pinned workflows make these PR jobs applicable: test-debt; Windows rust-regression; Linux/macOS Rust regression; the four-host terminal-snapshot matrix; windows-release-cli-smoke; frontend-regression; and lockfile-drift, whose detector must report no package-manifest change. Bundle/version path filters do not match. Re-derive only if relevant workflow/config/diff paths drift.

Branch protection is strict and currently requires validate-branch-name; the issue-numbered branch and open issue satisfy its intended input but the check itself still must pass. CI uses Node 22, npm 11.6.2, Rust stable, npm ci, and committed Cargo lock resolution. Local runs record resolved versions. The Linux primary-seam execution is owned exactly as specified above. Every triggered and configured-required check must pass on the exact PR-head SHA; another SHA, unexplained skip, zero-test result, bypass, or waiver is insufficient.

### Git, bounded drift, mutation, and recovery

- The immutable planning base remains 1eee2cd by release-owner ruling. Do not add a rebase prerequisite merely because main moved.
- Immediately before first product mutation and PR creation/update, fetch live main and classify drift from the pinned base by affected source/test/workflow/toolchain semantics. Refresh only materially affected evidence; unrelated motion cannot restart accepted design.
- Require the issue branch, recorded base/head, clean index, no unrelated tracked/untracked state, and frozen six-path scope. Product Git writes occur only in authorized repo-AgentsCommander and deliver through a PR, never direct to main.
- Before each write, recheck affected bytes. Recovery may unstage/remove only this run’s newly created or demonstrably unchanged output. Never use broad reset, restore, or clean.
- Test/build artifacts stay in repository-standard ignored roots or TempDirs. On Linux-test failure, restore the exact fixture mode before retaining/removing only fixture-owned paths.
- Final evidence enumerates tracked, staged, ordinary-untracked, lockfile, config, workflow, and intended path state.
- Hash plan certifications from git cat-file blob <commit>:plans/1577-writable-config-resolver.md, not worktree bytes or git show.

### Bounded diagnostics and enhanced controls

Tests/builds use runner/CI timeouts and retain exit/stdout/stderr. The Linux child uses explicit 15-second completion plus 5-second kill/reap bounds; timeout/cancellation is failure and cleanup cannot erase the primary failure.

No enhanced signing, binary-hash, helper inventory, hostile-parent quarantine, product ACL/mode mutation, reparse proof, custom transaction ledger, or exclusive cross-process lock applies. The copied-test fixture is isolated test setup for the reported Linux hazard; shared retries, exact-head CI, real subprocess acceptance, scoped diff, and no-clobber cleanup are proportionate.

## Ready/acceptance verdict

Implementation is accepted only when the precedence/cache contract, one-capture pure derivation, unchanged first-underscore/profile behavior, shared #1436 retry schedule, closed conclusive allowlist, marker-indeterminate hard error, explicitly inverted broken-link assertion, cleanup/debris diagnostic, exact messages, exit 1, nine legacy outcomes, six-path implementation scope, release public-override smoke, exact Linux subprocess evidence, dependency criterion, local checks, release-owner workflow prerequisite, and exact-head CI all pass.

Any request to add a shipped test-home seam, silently relocate on an unknown error, coerce marker ambiguity to absent, duplicate the retry policy, change suffix parsing, restore a Windows ACL end-to-end case, add #1137/#1594/AppImage behavior, or weaken bounded cleanup is a blocker back to the release owner.
