# #1932: remove the `portable.txt` marker

Status: READY_FOR_IMPLEMENTATION
Round: 1.
Issue: [#1932](https://github.com/mblua/AgentsCommander/issues/1932). Branch: `fix/1932-remove-portable-txt-marker`. Base: `5203c4e3b0b5909fdc12337194c886b1514966c3` (`origin/main`, tag `v0.34.0`).
Class: removal. One behavior decision is taken here (D1); no architectural question is open.
Owner: Rust, plus the CI steps that pin one #1850 lib test name and the seven #1850 case records.
Depends on: #1930 P1/P2 and #1935 (merged; PR #2020, commit `7bc02092`, contained in `v0.33.0` and `v0.34.0`).
Files (16 entries, one rename): `src-tauri/src/config/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/tests/issue_1850_default_root.rs`, `scripts/smoke-cli-release-windows.ps1`, `scripts/pack-windows-portable.mjs`, `scripts/smoke-windows-portable.ps1`, `packaging/windows/PORTABLE.txt` → `packaging/windows/README.txt`, `.github/workflows/pr-regression-gates.yml`, `docs/features/portable-instances.md`, `docs/reference/directory-layout.md`, `docs/reference/settings.md`, `docs/reference/architecture.md`, `docs/glossary.md`, `docs/faq.md`, `npm/README.md`, `plans/1932-remove-portable-txt-marker.md`.

## Objective

Under the #1930 rule (`agentscommander.exe` selects `$HOME/.agentscommander`; `agentscommander_<suffix>.exe` selects its adjacent `.agentscommander_<suffix>` and refuses otherwise, never HOME) the `portable.txt` marker no longer changes any selection outcome. Remove the marker from configuration selection: its probe and failure operations, the `marker_path` field and the marker clause in the startup message, every test that exercises it, the Windows release smoke case that builds it, the portable-package readme that ships under its filename, and every documentation statement about `main` that names it. Keep the #1930 selection rule, both startup refusals, probe retry/classification, `instance_base`, and the #1850 proof.

## Facts at base (base `5203c4e3`; all line numbers are from that commit)

Evidence method: the indexed graph is a `v0.33`-era generation (`check_index_coverage` reports `metadata_changed` for every cited source file), so every line below was confirmed by reading or `grep`-ing the working tree directly.

- F1. Accepted decisions already in force: #1930 P2 (`plans/1930-retire-agentscommander-new/p2-suffixed-refusal.md`) added `ConfigStartupError::AdjacentDirectoryUnwritable` for a conclusively unwritable unmarked adjacent candidate and kept `AdjacentSelectionBlocked` for the marked and indeterminate refusals; #1935 (`7bc02092`) removed the suffixed HOME fallback. `git tag --contains 7bc02092` = `v0.33.0`, `v0.34.0`; base `5203c4e3` is v0.34.0.
- F2. Marker production surface, `src-tauri/src/config/mod.rs`: `ProbeOperation` variants `MarkerEntryMetadata` / `MarkerTargetMetadata` / `UnsupportedMarkerEntry` (162-164); `ProbeFailure::unsupported_marker` (200-211); the three marker arms of `ProbeFailure::reason` (215-227); `MarkerProbeOutcome` (257-262); `ConfigStartupError::AdjacentSelectionBlocked { config_dir, marker_path, reason }` (333-337) and its `Portable marker path` clause in `Display` (365-371); `MarkerEntryKind` (396-401) and `marker_entry_kind` (403-414); `probe_portable_marker_with` (416-474); `probe_portable_marker` (476-484); `AdjacentPaths.marker_path` (662) and its construction at 671; `blocked_adjacent_location`'s `marker_path` parameter (711-724); the marker match in `resolve_instance_location` (786-849) and `resolve_instance_location_with_probes` (890-898); the `probe_portable_marker` argument at the single `instance_location()` call (914-935).
- F3. Resolver routes with the marker probe still present: public override, then test override, then unsuffixed/unusable executable → HOME; suffixed executable → `MarkerProbeOutcome` × `WriteProbeOutcome`: `Indeterminate(any)` → `AdjacentSelectionBlocked` (marker path shown), `NotRun` → `AdjacentSelectionBlocked`; `Present` + write Success → adjacent; `Present` + any write failure → `AdjacentSelectionBlocked` (marker path shown); `Absent` + write Success → adjacent; `Absent` + conclusive write failure → `AdjacentDirectoryUnwritable`; `Absent` + indeterminate write failure → `AdjacentSelectionBlocked` (no marker path); `Absent` + write `NotRun` → `AdjacentSelectionBlocked` (`write probe was not run for an unmarked configuration directory`, line 841); `Present` + write `NotRun` → `AdjacentSelectionBlocked` (`write probe was not run for a portable configuration directory`, line 809). A leftover marker is matched case-insensitively on Windows and default macOS, so F12's packaging readme is a marker there.
- F4. `AdjacentSelectionBlocked`'s exact unmarked Display bytes are pinned by the `unmarked_error` case of `issue_1577_startup_and_diagnostic_formatters_are_exact`: `AgentsCommander cannot start because configuration directory "{config_dir}" could not be safely selected: {reason}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.` (the `.` is appended unless `reason` already ends with one). `AdjacentDirectoryUnwritable`'s Display and the `#1930` message stay byte-identical.
- F5. `resolve_instance_location` and every marker type are private to `config/mod.rs`: `git grep -n 'resolve_instance_location\|MarkerProbeOutcome\|probe_portable_marker\|AdjacentSelectionBlocked'` outside that file matches nothing except `lib.rs`'s use of `config_startup_error()` at 925 and the `AdjacentDirectoryUnwritable` match at 4284-4287.
- F6. Tests at base that mention the marker: in `config/mod.rs` the helpers `marker_failure` (1012-1022) and `expected_marker` (1024-1030); the marker arguments in every resolver test; the marker-only tests `issue_1577_marker_present_success_selects_adjacent` (1293), `issue_1577_marker_present_any_write_failure_is_hard` (1308), `issue_1577_marker_absent_files_directories_and_symlinks` (1625), `issue_1577_real_marker_contents_are_never_interpreted` (1664), `issue_1577_marker_metadata_retries_windows_sharing_violation` (1677), `issue_1577_marker_metadata_permission_and_unsupported_are_indeterminate` (1700), `broken_marker_link_is_indeterminate_and_never_falls_home` (1730); `never_marker` (2167); the marker loop and marker argument in `issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome` (2013-2097); the marker closure in `issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once` (2224-2373). In `src-tauri/src/lib.rs`, `issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start` builds `case_root.join("portable.txt")` at 4320 and checks it at 4339 and 4486. In `src-tauri/tests/issue_1850_default_root.rs`, `PORTABLE_MARKER` (36), `PORTABLE_MARKER_BYTES` (65), `Case.portable` (292), the `canonical-absent-with-marker` case (344-351), the marker fixture (596-599), the `ISSUE1850_CASE_FIXTURES` field (629-636), the marker snapshot (644) and the marker preservation check (731-737).
- F7. CI pins names, so a rename or a deleted case must be updated in the same commit: `config::tests::issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once` at `.github/workflows/pr-regression-gates.yml` 141, 602, 842, 1299, 1694, 2151; `canonical-absent-with-marker` at 1342, 2190, 2397; `cases=8` at 1346, 2194, 2406; the unique-count checks at 1345, 2193, 2405; the "eight" comments/step name at 1325, 2300, 2358, 2409. `issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start` is pinned at 731 and its name is kept. `windows-release-cli-smoke` runs `npm run smoke:cli-release-windows` (2282).
- F8. Smoke gate at base (`scripts/smoke-cli-release-windows.ps1` 131-291): copies `$binaries[0]` (`agentscommander.exe`) to the suffixed `agentscommander_issue1577_cli.exe`, builds a broken NTFS junction at `bin\portable.txt`, runs `list-peers-lean`, and requires exit 1, byte-empty stdout and the exact marker-indeterminate stderr. It is the only CI execution of a suffixed release binary's startup refusal on Windows; nothing else asserts HOME residue on that platform.
- F9. Measured on this Windows host, `rustc 1.97.1`: `fs::create_dir_all` on an existing regular file returns `kind=AlreadyExists`, `raw_os_error=Some(183)`, message `Cannot create a file when that file already exists. (os error 183)`; `retry_transient_io_with_platform` retries only `Interrupted` and Windows raw 5/32, so that failure is `attempts = 1` and classifies `Indeterminate`. A regular file at the adjacent candidate path is therefore a deterministic, ACL-free and junction-free Windows fixture for the indeterminate refusal.
- F10. Marker inventory for `git grep -n 'portable\.txt'` outside this plan: 7 lines in `plans/1577-writable-config-resolver.md` (26, 69, 79, 356, 368, 384, 400; historical, unchanged), 0 lines in `docs/releases/**`, and the live doc lines named in the documentation matrix below. Creators of a file at the marker path: the CI smoke gate (F8, a broken junction, test-only) and, until D8, `scripts/pack-windows-portable.mjs` (F12, the portable zip readme). No installer or npm script writes the marker.
- F11. A leftover `portable.txt` on disk is inert after this change: nothing reads it, so it needs no migration, no deletion and no error. `docs/releases/**` and `plans/**` are not rewritten.
- F12. Windows portable zip readme: `scripts/pack-windows-portable.mjs` renders `packaging/windows/PORTABLE.txt` (replacing `{{VERSION}}`) into the zip as `PORTABLE.txt` beside the canonical unsuffixed `agentscommander.exe` (lines 29, 105, 107, 117), and `scripts/smoke-windows-portable.ps1` asserts the exact zip inventory and the rendered readme (lines 144, 180-186). The readme predates the marker (`7c83aa54`, #1589, 2026-08-27; the marker's #1577 landed 2026-08-31) and is user documentation, not a selection input, but on a case-insensitive filesystem its name is a marker path for a suffixed executable. Pack and smoke are driven by `bundle-validation.yml` on matching PRs and by `release.yml`; no doc or workflow names the file.

## Decisions (final, inlined)

- D1. The indeterminate branch keeps `AdjacentSelectionBlocked`, and the variant loses only its `marker_path` field. Rationale: under #1930 the unmarked-indeterminate route already produced exactly this variant and message with `marker_path: None` (F3), the exact bytes are already pinned by a test and already described by the docs, and the two refusals keep distinct, honest remedies (conclusive → move the executable or set the override; indeterminate → set the override). Collapsing both into `AdjacentDirectoryUnwritable` would change the indeterminate message and drop "rather than guessing" without removing a branch. Unchanged: suffixed write probe Success → adjacent candidate plus `instance_base`; `ConclusiveUnwritable` → `AdjacentDirectoryUnwritable`; unsuffixed or unusable executable, or any effective override → unchanged with probes never run; a suffixed executable still never selects HOME.
  Behavior deltas, all only on a machine that still has a leftover `portable.txt` (case-insensitive on Windows/default macOS) or for a direct caller:
  - A conclusive write failure previously took the marked route `AdjacentSelectionBlocked` (base 792-811, message ends `Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.` plus the marker path) and now reports `AdjacentDirectoryUnwritable` (`…cannot write its configuration directory "…" next to the executable: … Move the executable to a writable folder, or set AGENTSCOMMANDER_CONFIG_DIR…`). Refusal and never-HOME are unchanged; variant and message change.
  - An indeterminate write failure keeps the same variant; its message loses the `Portable marker path` clause.
  - An indeterminate marker state (broken symlink, metadata error) no longer blocks before the write probe; the write probe now decides, so a writable folder starts instead of refusing.
  - The single `NotRun` arm (production-unreachable; direct callers only) always says `write probe was not run for a portable configuration directory` (line 809). The `Absent` route previously said `write probe was not run for an unmarked configuration directory` (line 841); that string changes.
- D2. Marker types, probe functions and failure operations are deleted, not kept dormant: `MarkerProbeOutcome`, `MarkerEntryKind`, `marker_entry_kind`, `probe_portable_marker_with`, `probe_portable_marker`, `ProbeFailure::unsupported_marker`, the three marker `ProbeOperation` variants and their `reason()` arms, `AdjacentPaths.marker_path`, `blocked_adjacent_location`'s `marker_path` parameter, and `ConfigStartupError::AdjacentSelectionBlocked::marker_path` with its Display clause. `-D warnings` requires the deletions; removing `marker_entry_kind` also leaves the `std::fs::Metadata` import unused (line 36), so that import is deleted too.
- D3. Tests: delete the marker-only tests and fixtures; rewrite the affected resolver tests by dropping the marker argument; keep every selection, override, classifier, retry, write-probe and #1850 assertion. The one CI-pinned name that claims marker-first behavior is renamed to `issue_1850_lazy_helper_probes_suffixed_routes_write_once` and the six workflow name lists are updated. No other test is renamed.
- D4. The `#1850` integration test drops the `canonical-absent-with-marker` case (its only unique property was the removed reader) and its `portable` fixture; the remaining seven cases, the mechanism control, the refusal-only route and every assertion stay. The case count 8 → 7 is updated in the three workflow count guards and printed automatically by the test (for the 7-case runs).
- D5. The Windows smoke case is rewritten, not removed: same copied suffixed release binary and same `list-peers-lean` invocation, but the candidate path is created as a regular file (F9) instead of a broken marker junction, so the release build must refuse with the exact indeterminate message, exit 1, byte-empty stdout, an unchanged snapshot and no residue. It is the only Windows release CI proof of the suffixed refusal (F8) and it stops exercising the removed feature. Function, case directory, fixture prefix and PASS/FAIL text are renamed to the adjacent-refusal wording.
- D6. Documentation: every statement about `main` that names the marker is rewritten to the marker-free rule; every statement already scoped to a past release (`v0.30.3`, `v0.30.5`, `v0.31.0`, `v0.32.0`) stays. No new marker mention is added. The remaining live `portable.txt` lines are exactly the six version-scoped/cleanup lines in `docs/features/portable-instances.md` (18, 53, 54, 90, 122, 141) and `docs/reference/directory-layout.md:20`; the documentation matrix below gives the reason per line.
- D7. Out of scope: no CHANGELOG entry (release owner), no `docs/releases/**` or `plans/**` rewrite, no migration or deletion of an existing marker file, no change to the resolver precedence, `instance_base`, `ProbeFailureClass`, the classifier, the retry schedule, `AdjacentDirectoryUnwritable`, `scripts/smoke-cli-powershell.ps1`, `src-tauri/src/main.rs`, or any frontend/IPC file. The portable readme's stale storage text (it describes adjacent storage, but the unsuffixed canonical executable has selected `$HOME/.agentscommander` since #1868) is a separate user-facing documentation defect and is not fixed here; only its filename changes (D8).
- D8. The Windows portable readme stops shipping under the marker's filename: `packaging/windows/PORTABLE.txt` is renamed to `packaging/windows/README.txt` (content unchanged), `scripts/pack-windows-portable.mjs` writes `README.txt` into the zip, and `scripts/smoke-windows-portable.ps1`'s exact-inventory assertion and readme checks are updated. Rationale: the file is user documentation that predates the marker (F12), but its name is the marker path for a suffixed executable on a case-insensitive filesystem, and the readme itself tells the user to rename the executable; keeping the name would leave the removed concept visible in every portable download. No workflow or doc names the file, so the rename is confined to the template, the pack script and the smoke.

## Exact edits

Base line numbers; edit bottom-up. `src-tauri/**` is LF; `.github/workflows/pr-regression-gates.yml`, `scripts/smoke-cli-release-windows.ps1`, `scripts/pack-windows-portable.mjs`, `scripts/smoke-windows-portable.ps1` and `packaging/windows/PORTABLE.txt` are stored as LF but checked out CRLF (verified with `git ls-files --eol`) — keep the working-tree CRLF, including across the rename. Run `cargo fmt --manifest-path src-tauri/Cargo.toml --all` once after editing.

### `src-tauri/src/config/mod.rs` — production

| Base lines | Change |
|---|---|
| 36 | Remove `Metadata` from the `use std::fs::{self, File, Metadata, OpenOptions};` import (`marker_entry_kind` at 403 was its only user). |
| 162-164 | Delete the three marker `ProbeOperation` variants. |
| 200-211 | Delete `unsupported_marker` and the blank line after it. |
| 215-227 | Delete the `MarkerEntryMetadata`, `MarkerTargetMetadata` and `UnsupportedMarkerEntry` arms of `ProbeFailure::reason`. |
| 257-262 | Delete `MarkerProbeOutcome` and the blank line after it. |
| 332-337 | `AdjacentSelectionBlocked { config_dir: PathBuf, reason: String },` with the doc comment below. |
| 341-374 | Match `Self::AdjacentSelectionBlocked { config_dir, reason }` and delete the trailing `if let Some(marker_path) = marker_path { … }` block. |
| 396-414 | Delete `MarkerEntryKind` and `marker_entry_kind` with their blank line. |
| 416-484 | Delete `probe_portable_marker_with`, `probe_portable_marker` and the blank lines around them. |
| 642 | In the `InstanceLocation` doc comment, change `with its marker/write table` to `with its write-probe table`; the rest of the line is unchanged. |
| 662 | Delete the `marker_path: PathBuf,` field of `AdjacentPaths`. |
| 671 | Delete the `marker_path: parent.join("portable.txt"),` initializer. |
| 711-724 | Replace `blocked_adjacent_location` with the version below. |
| 736-760 | Replace the resolver doc and signature with the version below. |
| 762-860 | Replace the resolver body with the version below. |
| 862-907 | Replace `resolve_instance_location_with_probes` with the version below. |
| 934 | In `instance_location()`, delete the `probe_portable_marker,` argument; keep `probe_candidate_write`. |

```rust
// 332-336
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigStartupError {
    /// #1577: an executable-adjacent configuration directory could not be
    /// selected because the write probe gave an indeterminate result (or did not
    /// run). #1930: a suffixed executable never falls back to HOME, so startup
    /// stops and the message names the only remedy.
    AdjacentSelectionBlocked { config_dir: PathBuf, reason: String },
    /// #1930: a suffixed executable's unmarked adjacent directory is
    /// conclusively unwritable. There is no HOME fallback, so startup stops and
    /// the message names both remedies.
    AdjacentDirectoryUnwritable { config_dir: PathBuf, reason: String },
}

// 341-374
            Self::AdjacentSelectionBlocked { config_dir, reason } => {
                write!(
                    formatter,
                    "AgentsCommander cannot start because configuration directory \"{}\" could not be safely selected: {}",
                    config_dir.display(),
                    reason
                )?;
                if !reason.ends_with('.') {
                    write!(formatter, ".")?;
                }
                write!(
                    formatter,
                    " Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart."
                )
            }

// 711-724
fn blocked_adjacent_location(
    paths: AdjacentPaths,
    local_dir_stem: String,
    reason: String,
) -> InstanceLocation {
    let startup_error = ConfigStartupError::AdjacentSelectionBlocked {
        config_dir: paths.config_dir.clone(),
        reason,
    };
    InstanceLocation {
        config_dir: Some(paths.config_dir),
        local_dir_stem,
        instance_base: paths.instance_base,
        startup_error: Some(startup_error),
    }
}

// 736-760 (doc + signature)
/// Pure resolver for [`InstanceLocation`]. All branching for the #1077 instance
/// base lives here so it can be unit-tested with injected inputs.
///
/// - `test_override`: the debug `AGENTSCOMMANDER_TEST_CONFIG_DIR` value when set
///   (only threaded through in debug builds). An absolute override selects the
///   config directory verbatim and its parent becomes the instance base; a
///   relative override selects the config directory verbatim but reports NO
///   portable base (never absolutized through CWD).
/// - `current_exe_result`: the outcome of `std::env::current_exe()`.
/// - `home_dir`: `dirs::home_dir()` for the HOME location.
///
/// #1868: after the overrides, an executable without an underscore suffix
/// returns the HOME location immediately. It ignores BUILD_PROFILE, install
/// location and whatever probe outcomes were supplied; no startup error and no
/// fallback diagnostic can arise on that route. Only suffixed executables reach
/// the adjacent write-probe table.
///
/// #1930: that table never selects HOME. A conclusively unwritable candidate
/// becomes `ConfigStartupError::AdjacentDirectoryUnwritable`; an indeterminate
/// write result keeps `ConfigStartupError::AdjacentSelectionBlocked`.
pub(crate) fn resolve_instance_location(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    write_probe: WriteProbeOutcome,
) -> InstanceLocation {

// 762-860 (body, replacing the `marker_probe` match)
    // Local agent dir stem: from the running executable only. Independent of the
    // debug override so `agent_local_dir_name()` keeps naming replica dirs after
    // the real binary, and falls back to "agentscommander" when unavailable.
    let local_dir_stem = current_exe_result
        .as_ref()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());

    if let Some(raw) = nonblank_override(public_override.as_ref()) {
        return override_location(raw, local_dir_stem);
    }

    if let Some(raw) = nonblank_override(test_override.as_ref()) {
        return override_location(raw, local_dir_stem);
    }

    // #1868: route on the same first-underscore parser the profile uses. An
    // unsuffixed executable never constructs adjacent candidates; it selects
    // the canonical HOME location before `adjacent_paths` is consulted.
    let suffixed_executable = current_exe_result
        .as_ref()
        .ok()
        .filter(|path| profile::binary_suffix_from_path(path).is_some());
    let Some(paths) = suffixed_executable.and_then(|path| adjacent_paths(path)) else {
        return home_location(home_dir, local_dir_stem);
    };

    // #1868/#1930: the adjacent table runs the write probe and never HOME.
    match write_probe {
        WriteProbeOutcome::Success => InstanceLocation {
            config_dir: Some(paths.config_dir),
            local_dir_stem,
            instance_base: paths.instance_base,
            startup_error: None,
        },
        WriteProbeOutcome::Failed(failure)
            if failure.class == ProbeFailureClass::ConclusiveUnwritable =>
        {
            // #1930: a suffixed executable never falls back to HOME.
            let startup_error = ConfigStartupError::AdjacentDirectoryUnwritable {
                config_dir: paths.config_dir.clone(),
                reason: failure.reason(),
            };
            InstanceLocation {
                config_dir: Some(paths.config_dir),
                local_dir_stem,
                instance_base: paths.instance_base,
                startup_error: Some(startup_error),
            }
        }
        WriteProbeOutcome::Failed(failure) => {
            blocked_adjacent_location(paths, local_dir_stem, failure.reason())
        }
        WriteProbeOutcome::NotRun => blocked_adjacent_location(
            paths,
            local_dir_stem,
            "write probe was not run for a portable configuration directory".to_string(),
        ),
    }
}

// 862-907 (doc + whole function)
/// #1868: the lazy production orchestration behind [`instance_location`].
/// Decides whether the adjacent probe runs at all, runs the write probe only
/// when a suffixed executable has an adjacent candidate, and hands the outcome
/// to [`resolve_instance_location`].
///
/// An effective override or an executable without an underscore suffix never
/// constructs the adjacent candidate and never invokes the probe; its outcome
/// stays `NotRun`. The probe is a closure so tests can drive this exact
/// production path with a counting or panicking probe and no filesystem. It
/// keeps no state and abstracts no I/O of its own.
fn resolve_instance_location_with_probes<WriteProbe>(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    mut write_probe: WriteProbe,
) -> InstanceLocation
where
    WriteProbe: FnMut(&Path) -> WriteProbeOutcome,
{
    let override_selected = nonblank_override(public_override.as_ref()).is_some()
        || nonblank_override(test_override.as_ref()).is_some();
    let adjacent = if override_selected {
        None
    } else {
        current_exe_result
            .as_ref()
            .ok()
            .filter(|path| profile::binary_suffix_from_path(path).is_some())
            .and_then(|path| adjacent_paths(path))
    };
    let write_outcome = match adjacent {
        None => WriteProbeOutcome::NotRun,
        Some(adjacent) => write_probe(&adjacent.config_dir),
    };

    resolve_instance_location(
        public_override,
        test_override,
        current_exe_result,
        home_dir,
        write_outcome,
    )
}
```

### `src-tauri/src/config/mod.rs` — tests

- Delete helpers `marker_failure` (1012-1022) and `expected_marker` (1024-1030), and `never_marker` (2167-2169) with its blank line.
- Delete the marker argument line in every resolver call: 1057, 1082, 1112, 1133, 1156, 1178, 1205, 1220, 1236, 1285, 1351, 1374, 1392, 1419, 1590, 1789, 2118, 2134, 2145, 2159, 2327, 2342; the four `probes.run(...)` marker arguments at 2266, 2275, 2284 and 2302 disappear with the rewrite of the last test below. In `issue_1577_public_override_beats_debug_and_probe_failures` also delete the local `let marker_failure = ProbeFailure::from_retry(…)` block (1250-1263) and its argument (1264).
- Delete these tests whole, with their `#[test]` attribute: `issue_1577_marker_present_success_selects_adjacent` (1293-1306), `issue_1577_marker_present_any_write_failure_is_hard` (1308-1334), `issue_1577_marker_absent_files_directories_and_symlinks` (1625-1662), `issue_1577_real_marker_contents_are_never_interpreted` (1664-1675), `issue_1577_marker_metadata_retries_windows_sharing_violation` (1677-1698), `issue_1577_marker_metadata_permission_and_unsupported_are_indeterminate` (1700-1728), `broken_marker_link_is_indeterminate_and_never_falls_home` (1730-1757).
- `issue_1577_unmarked_indeterminate_failure_never_relocates` (1386-1409): drop the marker argument and pin the exact variant:
  ```rust
        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert_eq!(
            loc.startup_error,
            Some(ConfigStartupError::AdjacentSelectionBlocked {
                config_dir: expected_adjacent(),
                reason: failure.reason(),
            })
        );
  ```
  with `let failure = failed_write(RetryPlatform::Other, ProbeOperation::CreateProbeFile, expected_adjacent().join("probe.tmp"), Error::new(ErrorKind::AlreadyExists, "collision"), 1);` held in a local and passed as `WriteProbeOutcome::Failed(failure.clone())`; keep the trailing "no HOME text" assertion.
- `issue_1577_non_directory_candidate_is_indeterminate_and_hard` (1774-1793): drop the marker argument and replace the final `assert!(loc.startup_error.is_some());` with `loc.config_dir == Some(temp.path().join(".agentscommander_issue1577"))` and the same exact `AdjacentSelectionBlocked { config_dir, reason: failure.reason() }` assertion (`failure.clone()` into the outcome).
- `issue_1577_startup_and_diagnostic_formatters_are_exact` (1902-2010): delete `let marker = PathBuf::from("bin/portable.txt");` and the marker-failure `reason()` assertion block; replace the `present_error` block (with `marker_path: Some(marker.clone())`) with one `blocked_error` case:
  ```rust
        let blocked_error = ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: candidate.clone(),
            reason: write_failure.reason(),
        };
        assert_eq!(
            blocked_error.to_string(),
            format!(
                "AgentsCommander cannot start because configuration directory \"{}\" could not be safely selected: {} Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                candidate.display(),
                write_failure.reason()
            )
        );
  ```
  Delete the now-duplicate `unmarked_error` block (1971-1983); keep both `AdjacentDirectoryUnwritable` assertions.
- `issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome` (2013-2097): delete the `marker_outcomes` closure (2057-2063), the `for marker in marker_outcomes()` loop, the marker argument, and `marker={marker:?} ` from the `context` string. The name stays (CI-pinned, F7).
- `issue_1850_overrides_keep_precedence_and_identity_over_canonical_home` (2100-2165): drop the five marker arguments; the final call keeps `WriteProbeOutcome::Success` and its assertions.
- `issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes` (2176-2221): drop the `never_marker,` argument from all three calls.
- `issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once` (2224-2373) becomes `issue_1850_lazy_helper_probes_suffixed_routes_write_once`: `Probes.calls` is `RefCell<Vec<PathBuf>>`, `run(&self, write: WriteProbeOutcome, home: Option<PathBuf>)` records only the write path, and there are exactly three cases: `Success` → one call at `expected_adjacent()`, adjacent config, no error; `Failed(conclusive)` → one call, `AdjacentDirectoryUnwritable { config_dir: expected_adjacent(), reason: conclusive.reason() }`; `Failed(indeterminate)` → one call, `AdjacentSelectionBlocked { .. }`. The marker-only and `Present` cases are deleted.
- Keep unchanged: `failed_write`, all four `issue_1577_cleanup_*` tests, `issue_1577_persistent_required_delete_retains_debris_diagnostic`, `issue_1577_unknown_cleanup_upgrades_conclusive_primary_to_indeterminate`, all `issue_1577_interrupted_*` / `*_windows_transient_*` / `*_non_windows_*` / `*_conclusive_classifier_*` tests, the two `#[cfg(windows)]` raw-5/raw-32 tests (minus their marker argument), and `issue_1850_*` names.

### `src-tauri/src/lib.rs`

In `issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start` (4267), keep the name and `TEST_NAME` (CI-pinned at workflow 731):

1. Delete `let marker = case_root.join("portable.txt");` (4320).
2. `if marker.exists() || adjacent.exists() {` → `if adjacent.exists() {`, and `"fixture not fresh marker={} adjacent={}"` → `"fixture not fresh adjacent={}"`, deleting the `marker.display(),` argument (4339-4343).
3. `if adjacent.exists() || marker.exists() {` → `if adjacent.exists() {`, and `"adjacent state appeared adjacent={} marker={}"` → `"adjacent state appeared adjacent={}"`, deleting the `marker.display()` argument (4486-4491).

### `src-tauri/tests/issue_1850_default_root.rs`

1. Delete `const PORTABLE_MARKER: &str = "portable.txt";` (36) and `const PORTABLE_MARKER_BYTES` (65).
2. Lines 18-19: `the eight cases` → `the seven cases`; line 363: `the eight injected-HOME cases, the eight real-profile cases` → `the seven injected-HOME cases, the seven real-profile cases`.
3. `Case`: delete `portable: bool,` (292), delete the `canonical-absent-with-marker` case (344-352) including its `portable: true`, delete every remaining `portable: false,` line, and change `const CASES: [Case; 8]` (295) to `[Case; 7]`. The surviving `canonical-absent-without-marker` case becomes the last entry.
4. Delete the marker fixture block (596-600).
5. `ISSUE1850_CASE_FIXTURES`: drop ` portable={}` from the format string and ` if case.portable { "present" } else { "absent" },` from the arguments (629-636).
6. Delete `let marker_before = snapshot(&marker)?;` (644) and the marker preservation check (731-737).
7. `legacyMarker`, `AC_ISSUE1850_DISPOSABLE_PROFILE`, the admission gate, `mechanism_control`, the refusal-only route and every other assertion are unchanged.

### `scripts/smoke-cli-release-windows.ps1`

Replace `Invoke-Issue1577MarkerGate` (131-291) with the function below and update the call site (483-491). `$marker`, `$junctionTarget`, the junction creation/removal and its reparse checks, the `markerPath` result property disappear; the copied binary name, `$candidate`, the spawn block, the timeout/reap handling, the snapshot roots, the forbidden-residue list and the probe-residue check stay.

```powershell
function Invoke-Issue1577AdjacentRefusalGate {
    param(
        [Parameter(Mandatory=$true)] [string]$BinaryPath,
        [Parameter(Mandatory=$true)] [string]$Token,
        [Parameter(Mandatory=$true)] [string]$Root,
        [Parameter(Mandatory=$true)] [string]$LogDir
    )

    $errors = New-Object System.Collections.Generic.List[string]
    $caseLogDir = Join-Path $LogDir "issue-1577-adjacent-refusal"
    New-Item -ItemType Directory -Force -Path $caseLogDir | Out-Null
    $fixtureRoot = Join-Path $Root "issue-1577-adjacent-$([guid]::NewGuid().ToString('N'))"
    $fixtureFull = [System.IO.Path]::GetFullPath($fixtureRoot)
    $allowedRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd([char[]]"\/") + [System.IO.Path]::DirectorySeparatorChar
    if (-not $fixtureFull.StartsWith($allowedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing fixture outside smoke root: fixture=$fixtureFull root=$allowedRoot"
    }

    $binDir = Join-Path $fixtureRoot "bin"
    $cliRoot = Join-Path $fixtureRoot "cli-root"
    $copiedBinary = Join-Path $binDir "agentscommander_issue1577_cli.exe"
    $candidate = Join-Path $binDir ".agentscommander_issue1577_cli"
    $stdoutPath = Join-Path $caseLogDir "stdout.txt"
    $stderrPath = Join-Path $caseLogDir "stderr.txt"
    $baselinePath = Join-Path $caseLogDir "snapshot-before.json"
    $afterPath = Join-Path $caseLogDir "snapshot-after.json"
    $commandPath = Join-Path $caseLogDir "command.txt"
    $exitCode = $null
    $stdout = ""
    $stderr = ""
    $timedOut = $false
    $processStarted = $false

    try {
        New-Item -ItemType Directory -Force -Path $binDir | Out-Null
        New-Item -ItemType Directory -Force -Path $cliRoot | Out-Null
        Copy-Item -LiteralPath $BinaryPath -Destination $copiedBinary
        if (Test-Path -LiteralPath $candidate) {
            throw "adjacent candidate was not fresh: $candidate"
        }
        # A regular file at the adjacent candidate path makes create_dir_all fail
        # with AlreadyExists (183): indeterminate, one attempt, no ACLs needed.
        [System.IO.File]::WriteAllText($candidate, "not a directory")

        $snapshotRoots = @{ bin = $binDir; cliRoot = $cliRoot }
        $before = Get-Issue1577TreeSnapshot -Roots $snapshotRoots
        Set-Issue1577LogText -Path $baselinePath -Text $before

        $psi = [System.Diagnostics.ProcessStartInfo]::new()
        $psi.FileName = $copiedBinary
        $psi.Arguments = "list-peers-lean --token `"$Token`" --root `"$cliRoot`""
        $psi.WorkingDirectory = $cliRoot
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.EnvironmentVariables.Remove("AGENTSCOMMANDER_CONFIG_DIR")
        $psi.EnvironmentVariables.Remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        Set-Issue1577LogText -Path $commandPath -Text "$copiedBinary $($psi.Arguments)"

        $process = [System.Diagnostics.Process]::new()
        $process.StartInfo = $psi
        if (-not $process.Start()) {
            throw "Process.Start returned false for $copiedBinary"
        }
        $processStarted = $true
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()

        if (-not $process.WaitForExit(15000)) {
            $timedOut = $true
            try { $process.Kill() } catch { $errors.Add("timeout kill failed: $($_.Exception.Message)") | Out-Null }
            if (-not $process.WaitForExit(5000)) {
                $errors.Add("timed-out child did not reap within 5 seconds: pid=$($process.Id)") | Out-Null
            }
        }

        if ($process.HasExited) {
            $exitCode = $process.ExitCode
        }
        if (-not $stdoutTask.Wait(5000)) {
            $errors.Add("stdout capture did not finish within 5 seconds") | Out-Null
        } else {
            $stdout = $stdoutTask.Result
        }
        if (-not $stderrTask.Wait(5000)) {
            $errors.Add("stderr capture did not finish within 5 seconds") | Out-Null
        } else {
            $stderr = $stderrTask.Result
        }
        Set-Issue1577LogText -Path $stdoutPath -Text $stdout
        Set-Issue1577LogText -Path $stderrPath -Text $stderr

        if ($timedOut) {
            $errors.Add("copied release CLI timed out after 15 seconds") | Out-Null
        }
        if ($exitCode -ne 1) {
            $errors.Add("expected exit code 1, got $exitCode") | Out-Null
        }
        $stdoutBytes = [System.Text.Encoding]::UTF8.GetByteCount($stdout)
        if ($stdoutBytes -ne 0) {
            $errors.Add("expected byte-empty stdout, got $stdoutBytes UTF-8 byte(s)") | Out-Null
        }

        $nativeReason = ([System.ComponentModel.Win32Exception]::new(183)).Message.Trim()
        if (-not $nativeReason.EndsWith(".")) {
            $nativeReason += "."
        }
        $osReason = "$nativeReason (os error 183)"
        $expectedStderr = "AgentsCommander cannot start because configuration directory `"$candidate`" could not be safely selected: write probe could not create configuration directory `"$candidate`" after 1 attempt(s): $osReason. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.`n"
        if ($stderr -cne $expectedStderr) {
            $errors.Add("stderr did not match the exact adjacent-refusal startup message") | Out-Null
        }
        foreach ($forbidden in @("[log] file logging to", "[instance-gitignore]", "panicked at", "stack backtrace:", '"ok":', '"peers":')) {
            if ($stderr.Contains($forbidden)) {
                $errors.Add("stderr contained forbidden startup residue: $forbidden") | Out-Null
            }
        }

        $after = Get-Issue1577TreeSnapshot -Roots $snapshotRoots
        Set-Issue1577LogText -Path $afterPath -Text $after
        if ($after -cne $before) {
            $errors.Add("bin/cli-root snapshot changed across the child invocation") | Out-Null
        }
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            $errors.Add("candidate file changed type or disappeared: $candidate") | Out-Null
        }
        $criticalNames = @("app.log", ".gitignore", "master-token.txt", "web-token.txt", "daemon.pid", "settings.json", "app-outbox-path.txt")
        foreach ($criticalName in $criticalNames) {
            if (@(Get-ChildItem -Force -Recurse -LiteralPath $binDir, $cliRoot -ErrorAction SilentlyContinue | Where-Object { $_.Name -ceq $criticalName }).Count -gt 0) {
                $errors.Add("critical startup artifact appeared: $criticalName") | Out-Null
            }
        }
        if (@(Get-ChildItem -Force -Recurse -LiteralPath $binDir, $cliRoot -ErrorAction SilentlyContinue | Where-Object { $_.Name -like ".agentscommander-write-probe-*.tmp" }).Count -gt 0) {
            $errors.Add("write-probe residue appeared") | Out-Null
        }
    } catch {
        $errors.Add($_.Exception.ToString()) | Out-Null
    } finally {
        if ($processStarted -and $null -ne $process) {
            $process.Dispose()
        }
        if (Test-Path -LiteralPath $fixtureRoot) {
            try {
                Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction Stop
            } catch {
                $errors.Add("fixture cleanup failed for ${fixtureRoot}: $($_.Exception.Message)") | Out-Null
            }
        }
    }

    [pscustomobject]@{
        binaryPath = $copiedBinary
        shell = $null
        status = $(if ($errors.Count -eq 0) { "passed" } else { "failed" })
        exitCode = $exitCode
        root = $fixtureRoot
        logDir = $caseLogDir
        reason = $(if ($errors.Count -eq 0) { $null } else { $errors -join " | " })
        stdoutPath = $stdoutPath
        stderrPath = $stderrPath
        snapshotBeforePath = $baselinePath
        snapshotAfterPath = $afterPath
        candidatePath = $candidate
        cliRoot = $cliRoot
    }
}
```

Call site replacement:

```powershell
$adjacentGate = Invoke-Issue1577AdjacentRefusalGate -BinaryPath $binaries[0] -Token $Token -Root $Root -LogDir $LogDir
$results.Add($adjacentGate) | Out-Null
if ($adjacentGate.status -eq "passed") {
    Write-Host "PASS: #1577 copied release CLI adjacent-refusal preflight" -ForegroundColor Green
    $passed++
} else {
    Write-Host "FAIL: #1577 copied release CLI adjacent-refusal preflight: $($adjacentGate.reason)" -ForegroundColor Red
    $failed++
}
```

### `packaging/windows/PORTABLE.txt` → `packaging/windows/README.txt`

`git mv packaging/windows/PORTABLE.txt packaging/windows/README.txt`; content unchanged. The renamed path is already covered by `bundle-validation.yml`'s `packaging/windows/**` filter.

### `scripts/pack-windows-portable.mjs`

| Base line | Change |
|---|---|
| 29 | `const TEMPLATE      = join(ROOT, 'packaging', 'windows', 'PORTABLE.txt');` → `… 'README.txt');` |
| 30 | After `const CANONICAL_EXE = 'agentscommander.exe';` add `const README_NAME   = 'README.txt';` |
| 105 | `` die('PORTABLE.txt still contains an unresolved placeholder after rendering.') `` → `` die(`${README_NAME} still contains an unresolved placeholder after rendering.`) `` |
| 107 | `writeFileSync(join(stageDir, 'PORTABLE.txt'), …)` → `writeFileSync(join(stageDir, README_NAME), …)` |
| 117 | `` console.log(`[pack-portable] contents: ${CANONICAL_EXE}, ${EXTRA_FILES.join(', ')}, PORTABLE.txt`) `` → `…, ${README_NAME}` |

### `scripts/smoke-windows-portable.ps1`

| Base line | Change |
|---|---|
| 144 | `$expected = @('agentscommander.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'PORTABLE.txt')` → `'README.txt'` |
| 180 | `$readme = Join-Path $work 'PORTABLE.txt'` → `'README.txt'` |
| 184 | `"PORTABLE.txt names version $ExpectedVersion"` → `"README.txt names version $ExpectedVersion"` |
| 186 | `"PORTABLE.txt has no unresolved placeholder"` → `"README.txt has no unresolved placeholder"` |

The exact-inventory assertion (144-153: every expected name present and no unexpected entry) fails if `PORTABLE.txt` survives or `README.txt` is missing, so the existing smoke proves the rename in `bundle-validation` and at release; its admission guard and the unsuffixed HOME-storage contract are untouched.

### `.github/workflows/pr-regression-gates.yml`

| Base line | Change |
|---|---|
| 141, 602, 842, 1299, 1694, 2151 | `config::tests::issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once` → `config::tests::issue_1850_lazy_helper_probes_suffixed_routes_write_once`. |
| 1325, 2300 | `run all eight cases` → `run all seven cases`. |
| 1342, 2190 | In the case list, delete ` canonical-absent-with-marker`; `canonical-absent-without-marker` stays. |
| 1345, 2193 | `-eq 8` → `-eq 7` and `expected 8 unique case records` → `expected 7 unique case records`. |
| 1346, 2194 | `cases=8` → `cases=7`. |
| 2358 | `IS #1850 real-profile eight-case proof` → `IS #1850 real-profile seven-case proof`. |
| 2397 | Delete `'canonical-absent-with-marker',` from the `$cases` array. |
| 2405 | `-ne 8` → `-ne 7` and `expected 8 unique case-success records` → `expected 7 unique case-success records`. |
| 2406 | `cases=8` → `cases=7`. |
| 2409 | `real-profile proof verified, 8 cases` → `real-profile proof verified, 7 cases`. |

Nothing else in the file changes; keep the CRLF working-tree line endings.

### Documentation matrix

Every line below is in the base tree. "Change" replaces the base text with the exact new text in the block that follows the table; "Keep" is untouched.

| File:line | Action | Text and reason |
|---|---|---|
| `docs/features/portable-instances.md:18` | Keep | `Release builds of v0.30.3 … do not inspect portable.txt …` — history of `v0.30.3`; still true. |
| `docs/features/portable-instances.md:26` | Change | The override skips the adjacent candidate and its write probe; no marker exists to skip on `main`. |
| `docs/features/portable-instances.md:27` | Change | The unsuffixed route skips the adjacent candidate and its probe; drop the marker. |
| `docs/features/portable-instances.md:28` | Change | The suffixed route probes the adjacent candidate; `main` no longer inspects a marker. |
| `docs/features/portable-instances.md:29-30` | Change | Replace the `Marker present` / `Marker absent` bullets with the three write-probe outcomes (D1). |
| `docs/features/portable-instances.md:32` | Delete | `A marker cannot override …` — the marker no longer exists on `main`. |
| `docs/features/portable-instances.md:53,54` | Keep | `portable.txt` in `$HOME/.agentscommander-new` history of `v0.30.5`-`v0.32.0`; still true. |
| `docs/features/portable-instances.md:90` | Keep | `v0.30.3` ignores `portable.txt`; still true. |
| `docs/features/portable-instances.md:91` | Change | Creating a marker is not part of the current rule; the instruction becomes "confirm no override, then fix the location or override on failure". |
| `docs/features/portable-instances.md:95` | Change | Drop "one marker can serve multiple binaries"; the file-stem rule and its consequences stay. |
| `docs/features/portable-instances.md:122` | Keep | `v0.30.3` AppImage statement; still true. |
| `docs/features/portable-instances.md:141` | Keep | Version-agnostic cleanup advice for a marker left by an older, verified resolver. |
| `docs/reference/directory-layout.md:20` | Keep | `v0.30.3` has no `portable.txt`; still true. |
| `docs/reference/directory-layout.md:21` | Change | The `v0.33.0`/`main` rule without the marker; both refusal messages named (D1). |
| `docs/reference/settings.md:17` | Change | Drop the marker from the `v0.33.0`/`main` scope sentence. |
| `docs/reference/settings.md:19` | Change | Drop the marker from both refusal messages. |
| `docs/reference/architecture.md:641` | Change | Drop the marker clauses and the word from the `v0.30.3` comparison. |
| `docs/reference/architecture.md:762` | Change | Drop `marker` from the `config/mod.rs` responsibility row. |
| `docs/glossary.md:109` | Change | Drop the marker clause and the stale "newer unpublished `main`" wording. |
| `docs/faq.md:23` | Change | Drop `marker` from the resolver-ownership sentence and drop the stale "unpublished". |
| `docs/faq.md:31` | Change | Drop the marker sentence; the version warning about the public override stays. |
| `npm/README.md:60` | Change | Drop the marker clause from the `v0.34.0` unsuffixed route. |
| `npm/README.md:66` | Keep | `portable-marker/write-probe rules` as historical npm `0.30.3` behavior. |

Exact replacement texts (replace the named base line, or the paragraph it starts, with the block content):

`docs/faq.md:23`:

```text
Locally, in the application config directory selected by the exact binary version. Published `v0.30.3` immediately selects an executable-adjacent directory when it can derive one; the public override and writability rules belong to `v0.33.0` and later resolvers. Projects keep their shared state in their own `.ac/` folder. Plain JSON, TOML, and markdown — every file is human-readable and `git diff`-able. See the versioned [config directory rule](features/portable-instances.md#config-directory-rule) and [`PRIVACY.md`](../PRIVACY.md).
```

`docs/faq.md:31`:

```text
Yes — that is what [portable instances](features/portable-instances.md) are for. Put each renamed raw executable in a writable location and confirm that its exact version selects a distinct adjacent directory: `v0.30.3` ignores the public override, and only a later resolver honors it. The suffix gives each instance a distinct mutex and web port.
```

`docs/features/portable-instances.md:26`:

```text
1. A nonblank `AGENTSCOMMANDER_CONFIG_DIR` selects its original value verbatim and skips the adjacent candidate and its write probe. An empty or whitespace-only value is ignored; prefer an absolute value so the selected path is unambiguous.
```

`docs/features/portable-instances.md:27`:

```text
2. Without the override, an executable without an underscore suffix, such as `agentscommander.exe`, uses `$HOME/.agentscommander`. It skips the adjacent candidate and its write probe. If the runtime cannot report a usable executable name, AC treats it the same way.
```

`docs/features/portable-instances.md:28`:

```text
3. An executable with an underscore suffix, `agentscommander_<suffix>.exe`, derives the adjacent candidate `<native-executable-folder>/.agentscommander_<suffix>` and probes its writability once. It never uses `$HOME`.
```

`docs/features/portable-instances.md:29-30` (the two bullets become three, same three-space indent):

```text
   - **Writable candidate:** a successful write probe selects the adjacent candidate.
   - **Conclusively unwritable candidate:** startup stops with `AgentsCommander cannot start because it cannot write its configuration directory "<candidate>" next to the executable: <reason>. Move the executable to a writable folder, or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, and restart.`
   - **Indeterminate write result:** startup stops rather than guessing, and the message tells you to set `AGENTSCOMMANDER_CONFIG_DIR` to a writable directory.
```

`docs/features/portable-instances.md:91`:

```text
   - For `v0.33.0` or a development build from `main`, confirm that no nonblank public override is present. If selection fails, move the tree to a writable location or set `AGENTSCOMMANDER_CONFIG_DIR`; a suffixed executable on `v0.33.0` or `main` has no home fallback.
```

`docs/features/portable-instances.md:95`:

```text
Under either verified resolver, each binary derives its adjacent candidate from its own file stem. Distinct selected directories isolate settings, sessions, logs, and tokens; the suffix separately determines the mutex and port.
```

`docs/reference/directory-layout.md:21`:

```text
- **`v0.33.0` and `main`:** a nonblank public override wins. Otherwise an executable without an underscore suffix, such as `agentscommander.exe`, uses `$HOME/.agentscommander` and never checks an adjacent directory. An executable with an underscore suffix, `agentscommander_<suffix>.exe`, probes the writability of its adjacent `.agentscommander_<suffix>` directory and uses only that directory, never `$HOME`. A conclusively unwritable directory stops startup, and the message tells you to move the executable to a writable folder or set `AGENTSCOMMANDER_CONFIG_DIR`. An indeterminate write result also stops startup, and that message tells you to set `AGENTSCOMMANDER_CONFIG_DIR`. Neither a `v0.33.0` nor a `main` build moves, copies or merges configuration from an older folder; each reads a folder only when this rule selects it. See [Settings left by published releases](../features/portable-instances.md#settings-left-by-published-releases).
```

`docs/reference/settings.md:17`:

```text
Published `v0.30.3` has no public override or writability probe; it does not fall back because a derivable adjacent path is read-only. The public override and writability-probe behavior are in `v0.33.0` and `main`; before relying on them for any other release, verify that exact release tag. See [Portable instances](../features/portable-instances.md#config-directory-rule) for the complete versioned contract.
```

`docs/reference/settings.md:19`:

```text
On `v0.33.0` and `main`, `agentscommander_<suffix>.exe` never uses `$HOME`. If its adjacent folder cannot be written, it does not start. A conclusively unwritable folder gives a message that tells you to move the executable to a writable folder or set `AGENTSCOMMANDER_CONFIG_DIR`; when the write result is indeterminate, the message tells you to set `AGENTSCOMMANDER_CONFIG_DIR`. Neither a `v0.33.0` nor a `main` build moves, copies or merges settings from an older folder; each reads a folder only when its own rule selects it. To find and reuse them, see [Settings left by published releases](../features/portable-instances.md#settings-left-by-published-releases).
```

`docs/reference/architecture.md:641`:

```text
The current `main` source selects the application config directory once at runtime: a nonblank public override wins; otherwise an executable without an underscore suffix uses `$HOME/.agentscommander`, and an executable with an underscore suffix uses only its successfully probed adjacent candidate, never the home directory. A conclusively unwritable candidate stops startup with a message that says to move the executable to a writable folder or set `AGENTSCOMMANDER_CONFIG_DIR`; an indeterminate write result also stops startup, with a message that says to set `AGENTSCOMMANDER_CONFIG_DIR`. `v0.33.0` is the first release with this resolver. `v0.30.3` immediately selects a derivable adjacent path and has none of those public override or probe branches. A renamed binary is isolated only when its exact version selects a distinct path. See [Directory layout](directory-layout.md#the-config-dir-selection-rule) for the full versioned contract.
```

`docs/reference/architecture.md:762` (the table row keeps its enclosing pipes):

```text
| `config/mod.rs` | `config_dir()`: current `main` override, adjacent-candidate, write-probe, and home-fallback resolution; inspect release tags for shipped behavior |
```

`docs/glossary.md:109`:

```text
A renamed raw executable (with an `_<suffix>` such as `agentscommander_team-a.exe`) verified to select a distinct adjacent config directory, plus its own mutex and web port. Published `v0.30.3` selects adjacency without a public override; `v0.33.0` and later resolvers add it and refuse startup when a suffixed executable's adjacent directory cannot be written. Project `.ac/` state is still shared when two instances register the same project.
```

`npm/README.md:60`:

```text
The `v0.34.0` native resolver selects a non-blank `AGENTSCOMMANDER_CONFIG_DIR` value verbatim when present. Otherwise, the normal unsuffixed npm executable selects the user's home directory plus `.agentscommander`, independently of its install location and build profile. It does not probe an adjacent configuration directory on this route. Suffixed executables use their adjacent `.agentscommander_<suffix>` directory and refuse to start when it is not writable.
```

## Behavior, edge cases and failure behavior

| Situation (no override, suffixed executable) | After #1932 | Evidence |
|---|---|---|
| adjacent candidate writable | adjacent `.agentscommander_<suffix>` + `instance_base` = executable folder; exit 0 | unchanged from #1930; `issue_1577_blank_overrides_fall_through_to_adjacent_success`, `issue_1850_lazy_helper_probes_suffixed_routes_write_once` |
| conclusively unwritable | exit 1, `AdjacentDirectoryUnwritable`, message names "move the executable" or the override; never HOME | unchanged; `issue_1930_unmarked_conclusive_failure_refuses_and_never_selects_home`, raw-5/raw-32 tests |
| indeterminate (non-directory candidate, cleanup failure, unknown error) | exit 1, `AdjacentSelectionBlocked { config_dir, reason }`; message is F4's exact bytes without the marker clause; never HOME | `issue_1577_unmarked_indeterminate_failure_never_relocates`, `issue_1577_non_directory_candidate_is_indeterminate_and_hard`, rewritten smoke gate |
| write probe not run (direct calls only) | `AdjacentSelectionBlocked`; the one arm says `write probe was not run for a portable configuration directory`, where the old `Absent` route said `unmarked configuration directory` | production cannot reach it; F3 |
| unsuffixed executable / unusable `current_exe()` | `$HOME/.agentscommander`, no probe, no error | unchanged; `issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome` |
| any effective override | override verbatim; no probe | unchanged; override tests |
| leftover `portable.txt`, candidate writable | no reader, no error, bytes untouched; adjacent candidate selected; no migration | this change; residue check |
| leftover `portable.txt`, conclusive write failure | `AdjacentDirectoryUnwritable`; the marked path used to report `AdjacentSelectionBlocked` with the marker path | refusal and never-HOME unchanged; this change |
| leftover `portable.txt`, indeterminate write failure | `AdjacentSelectionBlocked` without the marker path clause | variant unchanged; this change |
| leftover `portable.txt`, indeterminate or broken marker entry | marker ignored; the write probe decides, so a writable folder starts instead of refusing | this change |
| marker probe at startup | no marker probe exists | this change |

## Environment risk

- E1. The rewritten smoke gate's exact stderr depends on the Windows OS message for error 183; the script derives it from `Win32Exception` exactly as it did for error 2, so a localized runner stays consistent. Measured F9 pins kind, raw code, class and `attempts = 1`.
- E2. The workflow and the four portable/Windows scripts (including `packaging/windows/PORTABLE.txt`) are CRLF in the working tree; an edit that rewrites them with LF produces a whole-file diff. Preserve CRLF.
- E3. `cargo test --lib` deletes tests, so any external exact-count guard on the `config::` filter would move; the only exact count in CI belongs to the unrelated `screenshot::native` filter (line 795), which does not change.
- E4. The #1850 real-profile legs and the Linux 0555 leg cannot run on a Windows development host; they are CI-owned on the PR head.
- E5. `npm run smoke:cli-release-windows` needs `target/release/agentscommander.exe` and `agentscommander_testeable.exe`; run it where that build exists, otherwise let `windows-release-cli-smoke` prove it.
- E6. Renaming the readme changes the inner file list of every future portable zip, so its hash changes as it does on any pack; `bundle-validation` proves the pack and smoke on the PR head and `release.yml` reruns both at release. The readme content is untouched (D8).

## Verification (repo root, Git Bash on Windows; `L` = replica-local scratch)

```bash
set -euo pipefail
mkdir -p "$L"
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib config:: 2>&1 | tee "$L/config.log"
for t in packaged_absolute_executable_yields_portable_config_and_base renamed_executable_keeps_per_stem_config_and_base \
  absolute_debug_override_sets_config_verbatim_and_parent_base relative_debug_override_sets_config_but_no_base \
  blank_debug_override_is_ignored relative_executable_keeps_relative_config_but_no_base missing_parent_takes_home_fallback \
  current_exe_failure_uses_home_fallback_and_default_stem current_exe_failure_and_no_home_yields_none_config \
  issue_1577_public_override_beats_debug_and_probe_failures issue_1577_blank_overrides_fall_through_to_adjacent_success \
  issue_1930_unmarked_conclusive_failure_refuses_and_never_selects_home issue_1577_unmarked_indeterminate_failure_never_relocates \
  issue_1577_identical_inputs_produce_identical_complete_location issue_1577_non_directory_candidate_is_indeterminate_and_hard \
  issue_1577_real_write_probe_keeps_directory_and_leaves_no_probe_file issue_1577_startup_and_diagnostic_formatters_are_exact \
  issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome \
  issue_1850_overrides_keep_precedence_and_identity_over_canonical_home \
  issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes \
  issue_1850_lazy_helper_probes_suffixed_routes_write_once; do
  grep -qE "^test ([A-Za-z0-9_]+::)*${t} \.\.\. ok$" "$L/config.log" || { echo "missing or not ok: $t"; exit 1; }
done
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib issue_1850 -- --test-threads=1 2>&1 | tee "$L/1850-lib.log"
grep -qE '^test result: ok\. [1-9][0-9]* passed; 0 failed' "$L/1850-lib.log"
cargo test --locked --manifest-path src-tauri/Cargo.toml --test issue_1850_default_root -- --test-threads=1 --nocapture 2>&1 | tee "$L/1850-root.log"
grep -qE '^test result: ok\. 2 passed; 0 failed' "$L/1850-root.log"
grep -qE '^ISSUE1850_REFUSAL_ONLY' "$L/1850-root.log"        # Windows ordinary run
# On the Linux/macOS legs (CI): ISSUE1850_DEFAULT_ROOT_PROOF_OK cases=7, seven ISSUE1850_CASE_OK records.
```

Negative controls (apply, run `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib config::`, require the named tests on `... FAILED` lines, revert, prove `sha256sum` equals the pre-control value):

- N1. Conclusive arm returns `home_location(home_dir, local_dir_stem)`. Fails `issue_1930_unmarked_conclusive_failure_refuses_and_never_selects_home`, `issue_1850_lazy_helper_probes_suffixed_routes_write_once`, `issue_1577_windows_raw_5_exhaustion_is_conclusive_without_real_acl`.
- N2. Indeterminate arm selects the adjacent candidate (`WriteProbeOutcome::Success` behavior). Fails `issue_1577_unmarked_indeterminate_failure_never_relocates`, `issue_1577_non_directory_candidate_is_indeterminate_and_hard`, `issue_1850_lazy_helper_probes_suffixed_routes_write_once`.
- N3. Re-add ` Portable marker path: "{}".` to the `AdjacentSelectionBlocked` Display. Fails `issue_1577_startup_and_diagnostic_formatters_are_exact`.
- N4 (Windows, release binaries available). In the rewritten smoke gate, expect `$exitCode -ne 0` instead of 1; `npm run smoke:cli-release-windows` must FAIL with `expected exit code 0, got 1`. Revert.
- N5 (CI-only). Delete `canonical-absent-without-marker` from `CASES`; the `rust-regression-linux`, `rust-regression-macos` and `issue-1850-windows-profile` case guards go red (`missing case record canonical-absent-without-marker`). No local run reaches these legs.
- N6 (Windows, `bundle-validation` equivalent). Leave `pack-windows-portable.mjs` writing `PORTABLE.txt`; `smoke-windows-portable.ps1` fails `README.txt is in the zip` and `no unexpected entries`. CI-held, like N5.

Residue, scope and acceptance checks:

```bash
git grep -n "portable\.txt" -- . ':!plans' ':!docs/releases' | cut -d: -f1,2 > "$L/residue.txt"
diff - "$L/residue.txt" <<'EOF'
docs/features/portable-instances.md:18
docs/features/portable-instances.md:53
docs/features/portable-instances.md:54
docs/features/portable-instances.md:90
docs/features/portable-instances.md:122
docs/features/portable-instances.md:141
docs/reference/directory-layout.md:20
EOF
test -z "$(git grep -nE 'MarkerProbeOutcome|MarkerEntryKind|marker_entry_kind|probe_portable_marker|unsupported_marker|marker_path|PORTABLE_MARKER|case\.portable|canonical-absent-with-marker|marker_first_then_write_once' -- src-tauri/src/config/mod.rs src-tauri/src/lib.rs src-tauri/tests/issue_1850_default_root.rs scripts/smoke-cli-release-windows.ps1 .github/workflows/pr-regression-gates.yml)"
test -z "$(git grep -in 'portable\.txt' -- src-tauri scripts packaging .github)"    # covers PORTABLE.txt (F12)
git grep -c 'portable\.txt' -- plans docs/releases
test "$(git grep -n 'issue_1850_lazy_helper_probes_suffixed_routes_write_once' -- .github/workflows/pr-regression-gates.yml | wc -l)" = "6"
test "$(git grep -n 'cases=7' -- .github/workflows/pr-regression-gates.yml | wc -l)" = "3"
```

The `diff` file has the 7 live lines named by D6 and the table; the plan file itself is under `plans/` and is excluded from the residue claim. `plans/**` holds 7 historical `portable.txt` lines (F10) and `docs/releases/**` holds none.

## CI evidence on the exact PR head

- `rust-regression` (Windows): clippy, the full suite, the focused #1850 debug and release steps with the renamed pinned name, the refusal-only integration step.
- `rust-regression-linux`: clippy, the exact-name 0555 test (name kept), the focused #1850 steps, then the seven-case default-root proof.
- `rust-regression-macos`: clippy and the seven-case proof.
- `windows-release-cli-smoke`: the rewritten adjacent-refusal gate (exit 1, exact stderr, unchanged snapshot, no residue) plus the unchanged public-override case.
- `bundle-validation` (Windows, triggered by the changed `scripts/**` and `packaging/**` paths): packs the portable zip and runs `smoke-windows-portable.ps1`, whose exact-inventory assertion proves `README.txt` replaced `PORTABLE.txt`.
- `issue-1850-windows-profile` (debug and release): `ISSUE1850_WINDOWS_PROFILE_PROOF_OK cases=7` on the real profile.

## Acceptance criteria

1. All verification commands pass; N1-N4 fail as listed and revert byte-exactly; N5 and N6 are CI-held guards that go red when their fixture is reverted.
2. `git grep -n "portable.txt"` returns only the 7 live lines of D6, the 7 `plans/1577-*` historical lines and this plan file's own mentions; no line describes `main`, and `git grep -in 'portable\.txt' -- src-tauri scripts packaging .github` is empty.
3. Selection behavior for unsuffixed, overridden and suffixed executables is unchanged from #1930/#1935, proven by the tests listed above; a suffixed executable never writes to HOME on any failure path.
4. CI is green with every name/count guard updated in the same commit; no unrelated check is weakened — the Windows smoke gate still fails on a wrong exit code, stderr, or residue.
5. A leftover `portable.txt` is inert; no migration code and no new file access is added.

## Preserve

`AdjacentDirectoryUnwritable` and its Display bytes; the `AdjacentSelectionBlocked` Display bytes without the marker clause (F4); `blocked_adjacent_location` after D2; `ProbeFailureClass`, `classify_io_error_for_platform`, `retry_transient_io_with_platform`, `probe_candidate_write*`, `WriteProbeFailure`, `WriteProbeOutcome`; `instance_base` and the project-path codec; override precedence; `profile::binary_suffix_from_path`; `module-arcs.txt`; `docs/releases/**`; `plans/**`; the pinned names `config::profile::tests::issue_1850_config_dir_name_table_is_profile_independent`, `config::tests::issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome`, `config::tests::issue_1850_overrides_keep_precedence_and_identity_over_canonical_home`, `config::tests::issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes`, `tests::issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start`; `legacyMarker` and the `AC_ISSUE1850_DISPOSABLE_PROFILE` admission gate; the `screenshot::native` count guard; every other workflow line; the packaging readme's content and `scripts/smoke-windows-portable.ps1`'s admission guard and unsuffixed HOME-storage contract; `bundle-validation.yml` and `release.yml` (unchanged).
